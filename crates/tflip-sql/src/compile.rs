//! Expressions resolved against the table they read and typed: [`Node`]s.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use tflip_storage::field::{Field, FieldType};

use crate::ast::{
    Aggregate, BinOp, CastType, Expr, ExprKind, OrderItem, OrderKey, ScoreSpec, Select, TrimSide,
    TypedLiteral,
};
use crate::error::{Pos, SqlError};
use crate::fts;
use crate::func::{self, Func};
use crate::types::{self, IntWidth, Ty};
use crate::value::V;

/// The table a statement reads, as expressions name it.
#[derive(Debug, Clone)]
pub struct Scope<'a> {
    /// The table's name as the statement wrote it, without a path or
    /// extension.
    pub name: &'a str,
    pub alias: Option<&'a str>,
    /// Its fields: a table's, or those of a `SELECT`'s results.
    pub fields: Cow<'a, [Field]>,
    /// The full-text rules of each field, by its place, for `CONTAINS`/`SCORE`;
    /// `None` where a field has no full-text index. Empty for a table whose
    /// full-text indexes are not known (a derived table, a non-`SELECT`).
    pub fts: std::rc::Rc<Vec<Option<crate::fts::Rules>>>,
    /// What `ROWID` and `EMPTY` need to know of the table.
    pub info: TableInfo,
}

/// What a table is, to expressions over its records.
#[derive(Debug, Clone, Default)]
pub struct TableInfo {
    /// The first twelve characters of its records' `ROWID`s
    /// ([`crate::rowid::prefix`]); none for a table that has no `ROWID`s,
    /// a derived table.
    pub rowid_prefix: Option<Arc<str>>,
    /// Whether it is a DBF table, whose empty fields `EMPTY` finds by their
    /// values, not as NULL.
    pub dbf: bool,
    /// Whether its text is in the client's OEM code page (an OEM table, see
    /// [`set_oem_text`]).
    pub oem: bool,
}

static OEM_TEXT: std::sync::OnceLock<fn(&str) -> String> = std::sync::OnceLock::new();

/// `text` as client text becomes OEM table text (see [`set_oem_text`]); as
/// it is where none was set.
pub fn oem_text(text: &str) -> String {
    OEM_TEXT.get().map_or_else(|| text.to_owned(), |f| f(text))
}

/// Sets how text the client wrote in a statement becomes the OEM table text
/// it is compared with and written as (best fit, as the client's writes), and
/// how OEM table text becomes the ANSI text the client reads.
pub fn set_oem_text(convert: fn(&str) -> String, back: fn(&str) -> String) {
    let _ = OEM_TEXT.set(convert);
    let _ = OEM_ANSI.set(back);
}

static OEM_ANSI: std::sync::OnceLock<fn(&str) -> String> = std::sync::OnceLock::new();

/// OEM table `text` as the ANSI text a client reads (see [`set_oem_text`]);
/// as it is where none was set.
pub fn oem_to_ansi_text(text: &str) -> String {
    OEM_ANSI.get().map_or_else(|| text.to_owned(), |f| f(text))
}

impl Scope<'_> {
    /// The full-text rules of the field at `index`, if it has a full-text index.
    pub fn fts_rules(&self, index: usize) -> Option<&crate::fts::Rules> {
        self.fts.get(index).and_then(Option::as_ref)
    }
}

/// A typed expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: Kind,
    pub ty: Ty,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Const(V),
    /// A field of the table, by index.
    Column(usize),
    /// `RECNO()`: the number of the record the row has at that column.
    RecNo(usize),
    /// `DELETED()`: whether that record is marked deleted.
    Deleted(usize),
    /// The `ROWID` pseudo-column of the record the row has at that column,
    /// from the table's prefix.
    RowId(usize, Arc<str>),
    /// A parameter, by number.
    Param(usize),
    Neg(Box<Node>),
    BitNot(Box<Node>),
    Not(Box<Node>),
    Binary(BinOp, Box<Node>, Box<Node>),
    IsNull(Box<Node>, bool),
    Between(Box<Node>, Box<Node>, Box<Node>, bool),
    In(Box<Node>, Vec<Node>, bool),
    Like(Box<Node>, Box<Node>, Option<Box<Node>>, bool),
    Case(Option<Box<Node>>, Vec<(Node, Node)>, Option<Box<Node>>),
    Func(Func, Vec<Node>),
    /// A call to a user-defined function of the dictionary, by its call name
    /// (`sq`, `util.pad`) and arguments.
    UserFunc(String, Vec<Node>),
    /// A conversion to the node's type, by `CAST` or `CONVERT`.
    Cast(Box<Node>, &'static str),
    Trim(TrimSide, Option<Box<Node>>, Box<Node>),
    /// A column of the row of an enclosing `SELECT`: how many levels out,
    /// and its index there.
    Outer(usize, usize),
    /// An aggregate of the group, by its number in the `SELECT`.
    Agg(usize),
    /// A subquery's value, by its number in the statement.
    Subquery(usize),
    /// `EXISTS (subquery)`.
    Exists(usize),
    /// `e [NOT] IN (subquery)`.
    InSelect(Box<Node>, usize, bool),
    /// `e op ALL|ANY (subquery)`: the operator, and whether `ALL`.
    Quantified(BinOp, Box<Node>, usize, bool),
    /// `CONTAINS(…)`: whether the full-text condition matches.
    Contains(Box<FtsSearch>),
    /// `SCORE`/`SCOREDISTINCT`: the full-text score; the flag is `SCOREDISTINCT`.
    Score(Box<FtsSearch>, bool),
}

/// A compiled `CONTAINS`/`SCORE`: what to tokenize and the condition to match.
#[derive(Debug, Clone, PartialEq)]
pub struct FtsSearch {
    /// The text to search.
    pub source: FtsSource,
    /// The rules the condition is parsed with and an expression source is
    /// tokenized with (a field's own rules, or the non-indexed default).
    pub rules: Arc<fts::Rules>,
    /// Whether the primary field has a full-text index (so the index keeps rows
    /// with no words, and a fully-optimized condition matches a NULL field).
    pub indexed: bool,
    /// Whether this is a `WHERE`/`JOIN` predicate the index answers over every
    /// row it keeps (so a NULL indexed field is a word-less row, not NULL); a
    /// `CONTAINS` in the select list is instead a scalar that is NULL for NULL.
    pub filter: bool,
    /// Whether the original engine answers this `CONTAINS` through the field's
    /// full-text order rather than the record's text: a `WHERE` part its AOF
    /// optimizes (see [`mark_answered`]). It matters for a FIXED order only,
    /// whose keys may no longer be its records' text.
    pub answered: bool,
    /// The condition.
    pub condition: FtsCondition,
    /// The condition as written, for plans (`CONTAINS(field,"<text>")`); empty
    /// for a non-constant or NULL condition.
    pub condition_src: String,
}

/// Marks the `CONTAINS` parts of the condition `node` that the original engine
/// answers through their full-text order ([`FtsSearch::answered`]): those
/// `answered` holds for, and under which it holds for every part — asked from
/// the whole condition down, through `AND`, `OR` and `NOT`.
pub fn mark_answered(node: &mut Node, answered: &mut dyn FnMut(&Node) -> bool) {
    if !answered(node) {
        return;
    }
    match &mut node.kind {
        Kind::Contains(search) => search.answered = true,
        Kind::Binary(BinOp::And | BinOp::Or, a, b) => {
            mark_answered(a, answered);
            mark_answered(b, answered);
        }
        Kind::Not(inner) => mark_answered(inner, answered),
        _ => {}
    }
}

/// Marks the `CONTAINS` parts of a static cursor's `WHERE` `node` that the
/// original engine answers through their full-text order: those `AND`ed at its
/// top with a constant condition, which it looks up as restrictions
/// (`fixtures/sql/fts_plans.json`); a `CONTAINS` under an `OR` or `NOT` is read
/// from each record's text.
pub fn mark_restrictions(node: &mut Node) {
    match &mut node.kind {
        Kind::Binary(BinOp::And, a, b) => {
            mark_restrictions(a);
            mark_restrictions(b);
        }
        Kind::Contains(search) if matches!(search.condition, FtsCondition::Const(_)) => {
            search.answered = true;
        }
        _ => {}
    }
}

/// Where a `CONTAINS`/`SCORE` reads its text.
#[derive(Debug, Clone, PartialEq)]
pub enum FtsSource {
    /// The full-text fields of `CONTAINS(*, …)`: each column (evaluated for its
    /// text) and the rules it tokenizes with.
    Fields(Vec<(Node, Arc<fts::Rules>)>),
    /// An expression (a field, or `field + field`), tokenized with the search's
    /// own rules.
    Expr(Box<Node>),
}

/// The `CONTAINS`/`SCORE` condition.
#[derive(Debug, Clone, PartialEq)]
pub enum FtsCondition {
    /// A constant condition, parsed once at compile time.
    Const(Arc<fts::Query>),
    /// A NULL condition: the result is NULL.
    Null,
    /// A condition that is a per-row value, parsed each row with the rules.
    Dynamic(Box<Node>),
    /// A constant condition of a `WHERE` that does not parse, with its native
    /// error: the original engine cannot set it as an AOF (a live cursor's
    /// plan says so) and fails as it evaluates it.
    Invalid(u32),
}

/// An aggregate function a `SELECT` computes for each group.
#[derive(Debug, Clone, PartialEq)]
pub struct AggSpec {
    pub func: Aggregate,
    /// The argument; `None` for `COUNT(*)`.
    pub arg: Option<Node>,
    pub distinct: bool,
    /// The result's type.
    pub ty: Ty,
    /// `GROUP_CONCAT`: the separator between values; `None` is the default
    /// comma.
    pub separator: Option<String>,
    /// `GROUP_CONCAT`: its inner `ORDER BY` keys, each with whether it
    /// descends; empty is record order.
    pub order: Vec<(Node, bool)>,
}

/// Where an expression is, for aggregate functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggContext {
    /// The select list, `HAVING` and `ORDER BY`.
    Allowed,
    Where,
    GroupBy,
    /// `INSERT`, `UPDATE` and `DELETE`.
    Statement,
}

/// A user-defined function the compiler can resolve: its call name (`sq`,
/// `util.pad`), the type it returns and how many parameters it takes.
#[derive(Debug, Clone)]
pub struct FunctionSig {
    pub name: String,
    pub ret: Ty,
    pub arity: usize,
    /// The types of its parameters, which an argument must be assignable to.
    pub params: Vec<Ty>,
}

/// A subquery the compiler has been told about: its number and its
/// columns' types.
#[derive(Debug, Clone)]
pub struct SubInfo {
    pub id: usize,
    pub columns: Vec<Ty>,
}

impl Node {
    pub(crate) fn new(kind: Kind, ty: Ty, pos: Pos) -> Node {
        Node { kind, ty, pos }
    }

    /// The node's value if it is a constant.
    pub fn constant(&self) -> Option<&V> {
        match &self.kind {
            Kind::Const(v) => Some(v),
            _ => None,
        }
    }

    /// Whether anything in the expression satisfies `f`.
    pub fn any(&self, f: &dyn Fn(&Node) -> bool) -> bool {
        if f(self) {
            return true;
        }
        let any = |n: &Node| n.any(f);
        match &self.kind {
            Kind::Const(_)
            | Kind::Column(_)
            | Kind::RecNo(_)
            | Kind::Deleted(_)
            | Kind::RowId(..)
            | Kind::Param(_)
            | Kind::Outer(..)
            | Kind::Agg(_)
            | Kind::Subquery(_)
            | Kind::Exists(_) => false,
            Kind::InSelect(a, ..) | Kind::Quantified(_, a, ..) => any(a),
            Kind::Neg(a)
            | Kind::BitNot(a)
            | Kind::Not(a)
            | Kind::IsNull(a, _)
            | Kind::Cast(a, _) => any(a),
            Kind::Binary(_, a, b) => any(a) || any(b),
            Kind::Between(a, b, c, _) => any(a) || any(b) || any(c),
            Kind::In(a, list, _) => any(a) || list.iter().any(any),
            Kind::Like(a, b, c, _) => any(a) || any(b) || c.as_deref().is_some_and(any),
            Kind::Case(a, whens, c) => {
                a.as_deref().is_some_and(any)
                    || whens.iter().any(|(w, t)| any(w) || any(t))
                    || c.as_deref().is_some_and(any)
            }
            Kind::Func(_, args) | Kind::UserFunc(_, args) => args.iter().any(any),
            Kind::Trim(_, a, b) => a.as_deref().is_some_and(any) || any(b),
            Kind::Contains(s) | Kind::Score(s, _) => {
                let in_source = match &s.source {
                    FtsSource::Expr(n) => any(n),
                    FtsSource::Fields(fields) => fields.iter().any(|(n, _)| any(n)),
                };
                let in_condition = matches!(&s.condition, FtsCondition::Dynamic(n) if any(n));
                in_source || in_condition
            }
        }
    }
}

impl Node {
    /// Renumbers the table's columns the expression reads: field `i` becomes
    /// `f(i)`.
    pub fn map_columns(&mut self, f: &dyn Fn(usize) -> usize) {
        let map = |n: &mut Node| n.map_columns(f);
        match &mut self.kind {
            Kind::Column(i) | Kind::RecNo(i) | Kind::Deleted(i) | Kind::RowId(i, _) => {
                *i = f(*i);
            }
            Kind::Const(_)
            | Kind::Param(_)
            | Kind::Outer(..)
            | Kind::Agg(_)
            | Kind::Subquery(_)
            | Kind::Exists(_) => {}
            Kind::InSelect(a, ..) | Kind::Quantified(_, a, ..) => map(a),
            Kind::Neg(a)
            | Kind::BitNot(a)
            | Kind::Not(a)
            | Kind::IsNull(a, _)
            | Kind::Cast(a, _) => map(a),
            Kind::Binary(_, a, b) => {
                map(a);
                map(b);
            }
            Kind::Between(a, b, c, _) => {
                map(a);
                map(b);
                map(c);
            }
            Kind::In(a, list, _) => {
                map(a);
                list.iter_mut().for_each(map);
            }
            Kind::Like(a, b, c, _) => {
                map(a);
                map(b);
                c.as_deref_mut().into_iter().for_each(map);
            }
            Kind::Case(a, whens, c) => {
                a.as_deref_mut().into_iter().for_each(map);
                for (w, t) in whens {
                    map(w);
                    map(t);
                }
                c.as_deref_mut().into_iter().for_each(map);
            }
            Kind::Func(_, args) | Kind::UserFunc(_, args) => args.iter_mut().for_each(map),
            Kind::Trim(_, a, b) => {
                a.as_deref_mut().into_iter().for_each(map);
                map(b);
            }
            Kind::Contains(s) | Kind::Score(s, _) => {
                match &mut s.source {
                    FtsSource::Expr(n) => map(n),
                    FtsSource::Fields(fields) => fields.iter_mut().for_each(|(n, _)| map(n)),
                }
                if let FtsCondition::Dynamic(n) = &mut s.condition {
                    map(n);
                }
            }
        }
    }
}

/// Compiles expressions against a scope, and types the parameters they
/// use.
pub struct Compiler<'a> {
    /// The tables of the row, whose columns are numbered on from one table
    /// to the next.
    tables: Vec<Scope<'a>>,
    /// The tables of the enclosing `SELECT`s' rows, innermost last.
    pub outer: Vec<Vec<Scope<'a>>>,
    /// Each parameter's type, as the expressions it is in decide.
    pub params: Vec<Ty>,
    /// The aggregates compiled so far.
    pub aggregates: Vec<AggSpec>,
    pub agg_context: AggContext,
    /// The aggregate being compiled, inside which another is an error.
    in_aggregate: Option<Aggregate>,
    /// The subqueries of the expressions, by where they are.
    pub subqueries: HashMap<*const Select, SubInfo>,
    /// The user-defined functions the compiler can resolve.
    pub functions: Vec<FunctionSig>,
    /// The `CONTAINS` predicates of the `WHERE`, in order, that `SCORE(n)`
    /// numbers; set before the select list and `ORDER BY` are compiled.
    pub where_contains: Vec<&'a Expr>,
    /// The connection's user, for `USER` / `USER()`: the login user, or the
    /// OS user of a free-table connection. The original engine folds `USER`
    /// to this string constant at prepare time, so its column is a
    /// `VARCHAR(len)`. `None` leaves `USER` unresolved (as a column).
    pub user: Option<String>,
    /// Whether the row's tables are all OEM ones, whose text the literals
    /// of the statement meet in the OEM code page (probed,
    /// `fixtures/sql/oem.json`).
    pub(crate) oem: bool,
    /// Whether the next expression compiled stands where a condition does
    /// (`WHERE`, `HAVING`, `ON`, `IIF`'s first argument), as an `AND`, an
    /// `OR` or a `NOT` passes on to its operands.
    cond: bool,
}

impl<'a> Compiler<'a> {
    /// A compiler for expressions over `scope` (none for `system.iota`)
    /// with `params` parameters.
    pub fn new(scope: Option<Scope<'a>>, params: usize) -> Compiler<'a> {
        Compiler::over(scope.into_iter().collect(), params)
    }

    /// A compiler for expressions over the row of `tables`.
    pub fn over(tables: Vec<Scope<'a>>, params: usize) -> Compiler<'a> {
        Compiler {
            oem: !tables.is_empty() && tables.iter().all(|t| t.info.oem),
            tables,
            outer: Vec::new(),
            params: vec![Ty::Unknown; params],
            aggregates: Vec::new(),
            agg_context: AggContext::Where,
            in_aggregate: None,
            subqueries: HashMap::new(),
            functions: Vec::new(),
            where_contains: Vec::new(),
            user: None,
            cond: false,
        }
    }

    /// `USER` / `USER()` as the connection user folded to a string constant,
    /// as the original engine prepares it; `None` when no user is set.
    fn user_const(&self, pos: Pos) -> Option<Node> {
        let user = self.user.as_ref()?;
        Some(Node::new(
            Kind::Const(V::Str(user.clone())),
            Ty::varchar(clamp(user.len())),
            pos,
        ))
    }

    /// The table, if the row is of one.
    pub fn scope(&self) -> Option<Scope<'a>> {
        match self.tables.as_slice() {
            [scope] => Some(scope.clone()),
            _ => None,
        }
    }

    pub fn tables(&self) -> &[Scope<'a>] {
        &self.tables
    }

    /// Where the columns of table `t` start in the row.
    pub fn offset(&self, t: usize) -> usize {
        self.tables[..t].iter().map(|s| s.fields.len()).sum()
    }

    /// Whether column `index` of the row is a field of a DBF table.
    fn is_dbf(&self, index: usize) -> bool {
        let mut start = 0;
        for scope in &self.tables {
            start += scope.fields.len();
            if index < start {
                return scope.info.dbf;
            }
        }
        false
    }

    /// Whether column `index` of the row is a ROWVERSION.
    fn is_rowversion(&self, index: usize) -> bool {
        let mut start = 0;
        for scope in &self.tables {
            if index < start + scope.fields.len() {
                return scope.fields[index - start].field_type == FieldType::RowVersion;
            }
            start += scope.fields.len();
        }
        false
    }

    /// The `ROWID` pseudo-column of the table `table` names, or of the only
    /// table that has one; `None` if there is none.
    fn rowid(&self, table: Option<&str>, pos: Pos) -> Result<Option<Node>, SqlError> {
        let tables: Vec<usize> = match table {
            Some(name) => Compiler::table_index(&self.tables, name)
                .into_iter()
                .collect(),
            None => (0..self.tables.len())
                .filter(|&t| self.tables[t].info.rowid_prefix.is_some())
                .collect(),
        };
        let [t] = tables[..] else {
            return match tables.len() {
                0 => Ok(None),
                _ => Err(SqlError::ambiguous_column("ROWID", pos)),
            };
        };
        Ok(self.tables[t].info.rowid_prefix.clone().map(|prefix| {
            let ty = Ty::Str {
                field: FieldType::Character,
                length: 18,
            };
            Node::new(Kind::RowId(self.offset(t), prefix), ty, pos)
        }))
    }

    /// The table of the row `name` names (its alias if it has one).
    pub fn table_index(tables: &[Scope<'_>], name: &str) -> Option<usize> {
        tables.iter().position(|scope| {
            scope
                .alias
                .map_or(scope.name.eq_ignore_ascii_case(name), |alias| {
                    alias.eq_ignore_ascii_case(name)
                })
        })
    }

    /// Compiles `expr`, which must have a type of its own.
    pub fn compile(&mut self, expr: &Expr) -> Result<Node, SqlError> {
        let node = self.node(expr)?;
        if node.ty == Ty::Unknown {
            return Err(match node.kind {
                Kind::Param(_) => SqlError::parameter_type(node.pos),
                _ => SqlError::null_type(),
            });
        }
        Ok(node)
    }

    /// Compiles `expr`, a value for a column of type `ty`: NULL and a
    /// parameter without a type of their own take `ty`.
    pub fn node_for(&mut self, expr: &Expr, ty: Ty) -> Result<Node, SqlError> {
        let mut node = self.node(expr)?;
        self.settle(&mut node, ty);
        Ok(node)
    }

    /// Compiles a condition: it must be logical.
    pub fn condition(&mut self, expr: &Expr) -> Result<Node, SqlError> {
        self.cond = true;
        let node = self.node(expr)?;
        if node.ty != Ty::Bool {
            return Err(match node.ty {
                Ty::Str { .. } | Ty::Bin { .. } => SqlError::invalid_operand("=", node.pos),
                _ => SqlError::not_boolean("=", node.pos),
            });
        }
        Ok(node)
    }

    /// Resolves a column name of the row; `table` is its qualifier.
    pub fn column(
        &self,
        table: Option<&str>,
        name: &str,
        pos: Pos,
    ) -> Result<(usize, Ty), SqlError> {
        match self.resolve(table, name, pos)? {
            (Kind::Column(i), ty) => Ok((i, ty)),
            _ => Err(SqlError::column_not_found(name, pos)),
        }
    }

    /// Resolves a column name in the row, or else in the rows of the
    /// enclosing `SELECT`s, innermost first.
    fn resolve(&self, table: Option<&str>, name: &str, pos: Pos) -> Result<(Kind, Ty), SqlError> {
        // A system variable (`::conn.Name`) that a host did not put in is
        // none: its scope is no table, and in a script no column either.
        if table.is_none()
            && name.starts_with("::")
            && let Some((scope, variable)) = name.split_once('.')
        {
            let at = pos + scope.chars().count() + 1;
            return Err(
                match self.tables.len() == 1 && self.tables[0].name.is_empty() {
                    true => SqlError::column_not_found(variable, at),
                    false => SqlError::table_not_found(scope, at),
                },
            );
        }
        let levels = std::iter::once(&self.tables).chain(self.outer.iter().rev());
        let mut first_error = None;
        for (depth, tables) in levels.enumerate() {
            let found = match table {
                Some(qualifier) => match Compiler::table_index(tables, qualifier) {
                    Some(t) => {
                        // The original engine points at the column after
                        // the qualifier.
                        let column = pos + qualifier.chars().count() + 1;
                        let fields = &tables[t].fields;
                        let i = fields
                            .iter()
                            .position(|f| f.name.eq_ignore_ascii_case(name))
                            .ok_or_else(|| SqlError::column_not_found(name, column))?;
                        Some((t, i))
                    }
                    None => {
                        let column = pos + qualifier.chars().count() + 1;
                        first_error.get_or_insert(SqlError::table_not_found(qualifier, column));
                        None
                    }
                },
                None => {
                    let mut found = None;
                    for (t, scope) in tables.iter().enumerate() {
                        if let Some(i) = scope
                            .fields
                            .iter()
                            .position(|f| f.name.eq_ignore_ascii_case(name))
                        {
                            if found.is_some() {
                                return Err(SqlError::ambiguous_column(name, pos));
                            }
                            found = Some((t, i));
                        }
                    }
                    found
                }
            };
            if let Some((t, i)) = found {
                let offset: usize = tables[..t].iter().map(|s| s.fields.len()).sum();
                let ty = Ty::of_field(&tables[t].fields[i]);
                let kind = if depth == 0 {
                    Kind::Column(offset + i)
                } else {
                    Kind::Outer(depth, offset + i)
                };
                return Ok((kind, ty));
            }
        }
        Err(first_error.unwrap_or_else(|| SqlError::column_not_found(name, pos)))
    }

    /// Gives an untyped parameter or NULL node the type `ty`.
    fn settle(&mut self, node: &mut Node, ty: Ty) {
        if node.ty != Ty::Unknown || ty == Ty::Unknown {
            return;
        }
        node.ty = ty;
        if let Kind::Param(i) = node.kind {
            self.params[i] = ty;
        }
    }

    fn node(&mut self, expr: &Expr) -> Result<Node, SqlError> {
        let cond = std::mem::take(&mut self.cond);
        let node = self.node_in(expr, cond)?;
        let logical = matches!(
            expr.kind,
            ExprKind::Not(_) | ExprKind::Binary(BinOp::And | BinOp::Or, ..)
        );
        // Where a condition stands, the original engine takes the numbers 0
        // and 1 for false and true and nothing else that is not logical
        // (`fixtures/sql/gram_cast2.json`).
        if !cond || logical || node.ty == Ty::Bool || node.ty == Ty::Unknown {
            return Ok(node);
        }
        match &expr.kind {
            ExprKind::Number(text) if text == "0" || text == "1" => Ok(Node::new(
                Kind::Const(V::Bool(text == "1")),
                Ty::Bool,
                expr.pos,
            )),
            ExprKind::Str(_) | ExprKind::Bin(_) => Err(SqlError::invalid_operand("=", expr.pos)),
            // A qualified column is pointed at after its qualifier.
            ExprKind::Column {
                table: Some(qualifier),
                ..
            } => Err(SqlError::not_boolean(
                "=",
                expr.pos + qualifier.chars().count() + 1,
            )),
            _ if matches!(node.ty, Ty::Str { .. } | Ty::Bin { .. }) => {
                Err(SqlError::invalid_operand("=", expr.pos))
            }
            _ => Err(SqlError::not_boolean("=", expr.pos)),
        }
    }

    fn node_in(&mut self, expr: &Expr, cond: bool) -> Result<Node, SqlError> {
        let pos = expr.pos;
        Ok(match &expr.kind {
            ExprKind::Number(text) => number(text, pos)?,
            ExprKind::Str(s) => {
                let length = s.chars().count().max(1);
                let text = match self.oem {
                    true => oem_text(s),
                    false => s.clone(),
                };
                Node::new(Kind::Const(V::Str(text)), Ty::varchar(clamp(length)), pos)
            }
            ExprKind::Bin(bytes) => Node::new(
                Kind::Const(V::Bin(bytes.clone())),
                Ty::Bin {
                    field: FieldType::VarBinary,
                    length: clamp(bytes.len().max(1)),
                },
                pos,
            ),
            ExprKind::Bool(b) => Node::new(Kind::Const(V::Bool(*b)), Ty::Bool, pos),
            ExprKind::Null | ExprKind::Default => Node::new(Kind::Const(V::Null), Ty::Unknown, pos),
            ExprKind::Typed(literal, text) => typed_literal(*literal, text, pos)?,
            ExprKind::Column { table, name } => {
                // Bare `USER` is the connection user unless a column is named
                // it (the original engine resolves a real column first).
                match self.resolve(table.as_deref(), name, pos) {
                    Ok((kind, ty)) => Node::new(kind, ty, pos),
                    Err(error) => match self.user_const(pos) {
                        Some(node) if table.is_none() && name.eq_ignore_ascii_case("USER") => node,
                        _ if name.eq_ignore_ascii_case("ROWID") => {
                            self.rowid(table.as_deref(), pos)?.ok_or(error)?
                        }
                        _ => return Err(error),
                    },
                }
            }
            ExprKind::Aggregate {
                func,
                arg,
                distinct,
                separator,
                order,
            } => self.aggregate(
                *func,
                arg.as_deref(),
                *distinct,
                separator.as_deref(),
                order,
                pos,
            )?,
            ExprKind::Subquery(select) => {
                let info = self.subquery(select)?;
                if info.columns.len() != 1 {
                    return Err(SqlError::subquery_columns());
                }
                Node::new(Kind::Subquery(info.id), info.columns[0], pos)
            }
            ExprKind::Exists(select) => {
                let info = self.subquery(select)?;
                Node::new(Kind::Exists(info.id), Ty::Bool, pos)
            }
            ExprKind::InSelect {
                expr,
                select,
                negated,
            } => {
                let e = self.node(expr)?;
                let info = self.subquery(select)?;
                if info.columns.len() != 1 {
                    return Err(SqlError::subquery_columns());
                }
                check_collations(&e, info.columns[0])?;
                if !comparable(e.ty, info.columns[0]) {
                    return Err(SqlError::invalid_operand("=", pos));
                }
                Node::new(
                    Kind::InSelect(Box::new(e), info.id, *negated),
                    Ty::Bool,
                    pos,
                )
            }
            ExprKind::Quantified {
                op,
                expr,
                select,
                all,
            } => {
                let e = self.node(expr)?;
                let info = self.subquery(select)?;
                if info.columns.len() != 1 {
                    return Err(SqlError::subquery_columns());
                }
                if !comparable(e.ty, info.columns[0]) {
                    return Err(SqlError::invalid_operand(op.symbol(), pos));
                }
                Node::new(
                    Kind::Quantified(*op, Box::new(e), info.id, *all),
                    Ty::Bool,
                    pos,
                )
            }
            ExprKind::Param(i) => Node::new(Kind::Param(*i), self.params[*i], pos),
            ExprKind::Neg(e) | ExprKind::Plus(e) => {
                let plus = matches!(expr.kind, ExprKind::Plus(_));
                let e = self.node(e)?;
                // A sign wants no parameter, whatever an earlier use of its
                // name typed it as (`fixtures/sql/params2.json`).
                if matches!(e.kind, Kind::Param(_)) {
                    return Err(SqlError::parameter_type(e.pos));
                }
                if !e.ty.is_numeric() {
                    // The original engine puts the error at the operand, but
                    // at the sign for an aggregate.
                    let at = if matches!(e.kind, Kind::Agg(_)) {
                        pos
                    } else {
                        e.pos
                    };
                    let sign = if plus { "+" } else { "-" };
                    return Err(match e.ty {
                        Ty::Bool => SqlError::not_signable(sign, at),
                        _ => SqlError::invalid_operand(sign, at),
                    });
                }
                if plus {
                    return Ok(e);
                }
                let ty = e.ty;
                Node::new(Kind::Neg(Box::new(e)), ty, pos)
            }
            ExprKind::BitNot(e) => {
                let e = self.node(e)?;
                if matches!(e.kind, Kind::Param(_)) {
                    return Err(SqlError::parameter_type(e.pos));
                }
                if doubled(&e) {
                    return Err(SqlError::invalid_operand_because(
                        "~",
                        "Values with decimals are not valid for bitwise NOT operator.",
                        e.pos,
                    ));
                }
                if matches!(e.ty, Ty::Num { scale, .. } if scale > 0) {
                    return Err(SqlError::invalid_operand_because(
                        "~",
                        "Values with decimals are not valid for bitwise NOT operator.",
                        e.pos,
                    ));
                }
                if e.ty == Ty::Bool {
                    return Ok(Node::new(Kind::Not(Box::new(e)), Ty::Bool, pos));
                }
                if !matches!(e.ty, Ty::Int { .. }) {
                    // A binary operand is where the original puts it.
                    let at = if matches!(e.ty, Ty::Bin { .. }) {
                        e.pos
                    } else {
                        pos
                    };
                    return Err(SqlError::invalid_operand("~", at));
                }
                let ty = e.ty;
                Node::new(Kind::BitNot(Box::new(e)), ty, pos)
            }
            ExprKind::Not(e) => {
                self.cond = cond;
                let e = self.node(e)?;
                if matches!(e.kind, Kind::Param(_)) {
                    return Err(SqlError::parameter_type(e.pos));
                }
                Node::new(Kind::Not(Box::new(truth(e))), Ty::Bool, pos)
            }
            ExprKind::Binary(op, a, b) => self.binary(*op, a, b, pos, cond)?,
            ExprKind::IsNull { expr, negated } => {
                let e = self.node(expr)?;
                // Whatever else types the parameter: the original engine
                // wants a type for the parameter itself
                // (`fixtures/sql/params.json`).
                if matches!(e.kind, Kind::Param(_)) {
                    return Err(SqlError::null_type());
                }
                Node::new(Kind::IsNull(Box::new(e), *negated), Ty::Bool, pos)
            }
            ExprKind::Between {
                expr,
                low,
                high,
                negated,
            } => {
                let mut e = self.node(expr)?;
                let mut low = self.node(low)?;
                let mut high = self.node(high)?;
                if [&e, &low, &high]
                    .iter()
                    .all(|n| matches!(n.kind, Kind::Param(_)))
                {
                    return Err(SqlError::null_type());
                }
                self.settle(&mut low, e.ty);
                self.settle(&mut high, e.ty);
                self.settle(&mut e, low.ty);
                if !comparable(e.ty, low.ty) || !comparable(e.ty, high.ty) {
                    return Err(SqlError::invalid_operand("BETWEEN", pos));
                }
                for bound in [&low, &high] {
                    if bound.kind_is_column() {
                        check_collations(&e, bound.ty)?;
                    }
                }
                Node::new(
                    Kind::Between(Box::new(e), Box::new(low), Box::new(high), *negated),
                    Ty::Bool,
                    pos,
                )
            }
            ExprKind::In {
                expr,
                list,
                negated,
            } => {
                let mut e = self.node(expr)?;
                // A parameter as the value of a list with parameters in it
                // has no type to take (`fixtures/sql/params2.json`).
                if matches!(e.kind, Kind::Param(_))
                    && list.iter().any(|i| matches!(i.kind, ExprKind::Param(_)))
                {
                    return Err(SqlError::null_type());
                }
                let mut items = Vec::with_capacity(list.len());
                for item in list {
                    // Only values: no column, expression or NULL.
                    let simple = match &item.kind {
                        ExprKind::Neg(inner) | ExprKind::Plus(inner) => {
                            matches!(inner.kind, ExprKind::Number(_))
                        }
                        // A script's variable or subquery value written back.
                        ExprKind::Cast { expr, .. } => {
                            matches!(expr.kind, ExprKind::Number(_) | ExprKind::Str(_))
                        }
                        kind => !matches!(
                            kind,
                            ExprKind::Null
                                | ExprKind::Column { .. }
                                | ExprKind::Binary(..)
                                | ExprKind::Func { .. }
                                | ExprKind::Case { .. }
                                | ExprKind::Not(_)
                                | ExprKind::BitNot(_)
                                | ExprKind::Between { .. }
                                | ExprKind::IsNull { .. }
                                | ExprKind::Like { .. }
                                | ExprKind::Cast { .. }
                                | ExprKind::Extract { .. }
                                | ExprKind::Trim { .. }
                                | ExprKind::Position(..)
                                | ExprKind::Aggregate { .. }
                        ),
                    };
                    if !simple {
                        return Err(SqlError::in_not_simple());
                    }
                    let mut item = self.node(item)?;
                    self.settle(&mut item, e.ty);
                    self.settle(&mut e, item.ty);
                    if !comparable(e.ty, item.ty) {
                        return Err(SqlError::invalid_operand("IN", item.pos));
                    }
                    items.push(item);
                }
                Node::new(Kind::In(Box::new(e), items, *negated), Ty::Bool, pos)
            }
            ExprKind::Like {
                expr,
                pattern,
                escape,
                negated,
            } => {
                let mut e = self.node(expr)?;
                let mut pattern = self.node(pattern)?;
                if matches!(e.kind, Kind::Param(_)) && matches!(pattern.kind, Kind::Param(_)) {
                    return Err(SqlError::null_type());
                }
                self.settle(&mut e, Ty::varchar(PARAM_TEXT));
                self.settle(&mut pattern, Ty::varchar(e.ty.length().max(1)));
                let escape = match escape {
                    Some(escape) => {
                        let mut escape = self.node(escape)?;
                        self.settle(&mut escape, Ty::varchar(1));
                        Some(Box::new(escape))
                    }
                    None => None,
                };
                if !e.ty.is_text() || !pattern.ty.is_text() {
                    return Err(SqlError::invalid_operand("LIKE", pos));
                }
                Node::new(
                    Kind::Like(Box::new(e), Box::new(pattern), escape, *negated),
                    Ty::Bool,
                    pos,
                )
            }
            ExprKind::Case {
                operand,
                whens,
                otherwise,
            } => self.case(operand.as_deref(), whens, otherwise.as_deref(), pos)?,
            // `USER()` folds to the connection user, a string constant.
            ExprKind::Func { name, args }
                if args.is_empty() && name.eq_ignore_ascii_case("USER") =>
            {
                match self.user_const(pos) {
                    Some(node) => node,
                    None => return Err(SqlError::no_function(name, pos)),
                }
            }
            // `RECNO` and `DELETED` take the alias of one of the tables.
            ExprKind::Func { name, args }
                if name.eq_ignore_ascii_case("RECNO") || name.eq_ignore_ascii_case("DELETED") =>
            {
                let deleted = name.eq_ignore_ascii_case("DELETED");
                let alias = match args.as_slice() {
                    [] => None,
                    [
                        Expr {
                            kind: ExprKind::Column { table: None, name },
                            ..
                        },
                    ] => Some(name.as_str()),
                    _ => return Err(SqlError::bad_argument(name, pos)),
                };
                let table = match (alias, self.tables.len()) {
                    (None, 0) => None,
                    (None, 1) => Some(0),
                    (None, _) => return Err(SqlError::table_not_unique()),
                    (Some(alias), _) => Some(
                        Compiler::table_index(&self.tables, alias)
                            .ok_or_else(|| SqlError::alias_not_found(alias))?,
                    ),
                };
                match (table.map(|t| self.offset(t)), deleted) {
                    (Some(i), false) => Node::new(Kind::RecNo(i), Ty::INTEGER, pos),
                    (Some(i), true) => Node::new(Kind::Deleted(i), Ty::Bool, pos),
                    (None, false) => Node::new(Kind::Const(V::Int(0)), Ty::INTEGER, pos),
                    (None, true) => Node::new(Kind::Const(V::Bool(false)), Ty::Bool, pos),
                }
            }
            // `LASTAUTOINC` takes `CONNECTION` or `STATEMENT`, as the parser
            // checked.
            ExprKind::Func { name, args } if name.eq_ignore_ascii_case("LASTAUTOINC") => {
                let func = match args.as_slice() {
                    [
                        Expr {
                            kind: ExprKind::Column { table: None, name },
                            ..
                        },
                    ] if name.eq_ignore_ascii_case("CONNECTION") => Func::LastAutoIncConnection,
                    [
                        Expr {
                            kind: ExprKind::Column { table: None, name },
                            ..
                        },
                    ] if name.eq_ignore_ascii_case("STATEMENT") => Func::LastAutoIncStatement,
                    _ => return Err(SqlError::bad_argument("LASTAUTOINC", pos)),
                };
                Node::new(Kind::Func(func, Vec::new()), Ty::INTEGER, pos)
            }
            // `NEWIDSTRING` takes its format as an *unquoted* word (as the SQL
            // engine does): none for delimited, or `D`/`B`/`P`/`N`/`M`/`MIME`/
            // `F` and the long names. A quoted string, a number, or an unknown
            // word is an invalid argument (2159).
            ExprKind::Func { name, args } if name.eq_ignore_ascii_case("NEWIDSTRING") => {
                let format = match args.as_slice() {
                    [] => "D",
                    [arg] => match &arg.kind {
                        ExprKind::Column { table: None, name } => sql_guid_format(name)
                            .ok_or_else(|| SqlError::bad_argument("NEWIDSTRING", pos))?,
                        _ => return Err(SqlError::bad_argument("NEWIDSTRING", pos)),
                    },
                    _ => return Err(SqlError::bad_argument("NEWIDSTRING", pos)),
                };
                let format = Node::new(Kind::Const(V::Str(format.to_owned())), Ty::varchar(1), pos);
                Node::new(
                    Kind::Func(Func::NewIdString, vec![format]),
                    Ty::varchar(38),
                    pos,
                )
            }
            ExprKind::Func { name, args } => {
                let mut nodes = Vec::with_capacity(args.len());
                for arg in args {
                    nodes.push(match &arg.kind {
                        ExprKind::Column {
                            table: None,
                            name: word,
                        } if func::takes_word(name, word) => Node::new(
                            Kind::Const(V::Str(word.clone())),
                            Ty::varchar(clamp(word.len())),
                            arg.pos,
                        ),
                        _ => {
                            self.cond = nodes.is_empty() && name.eq_ignore_ascii_case("IIF");
                            self.node(arg)?
                        }
                    });
                }
                match func::check(name, &mut nodes, pos) {
                    Ok((func, ty)) => {
                        // What is empty depends on the table's type.
                        let func = match nodes.first().map(|n| &n.kind) {
                            Some(Kind::Column(i)) if func == Func::Empty && self.is_dbf(*i) => {
                                Func::EmptyDbf
                            }
                            // The original gives the ANSI code of what an OEM
                            // table holds (`fixtures/sql/oemrows.json`).
                            _ if func == Func::Ascii && self.oem => Func::AsciiOem,
                            _ => func,
                        };
                        for node in &mut nodes {
                            if let Kind::Param(i) = node.kind
                                && node.ty != Ty::Unknown
                            {
                                self.params[i] = node.ty;
                            }
                        }
                        Node::new(Kind::Func(func, nodes), ty, pos)
                    }
                    // Not a built-in: a user-defined function of the dictionary.
                    Err(error) => {
                        let called = |s: &&FunctionSig| s.name.eq_ignore_ascii_case(name);
                        let Some(sig) = self
                            .functions
                            .iter()
                            .filter(called)
                            .find(|s| s.arity == nodes.len())
                        else {
                            return Err(match self.functions.iter().find(called) {
                                Some(sig) => SqlError::function_arguments(
                                    &sig.name,
                                    nodes.len() > sig.arity,
                                    pos,
                                ),
                                None => error,
                            });
                        };
                        for (i, (node, param)) in nodes.iter().zip(&sig.params).enumerate() {
                            if !crate::dml::assignable(*param, node.ty) {
                                return Err(SqlError::function_argument(&sig.name, i + 1, pos));
                            }
                        }
                        Node::new(Kind::UserFunc(sig.name.clone(), nodes), sig.ret, pos)
                    }
                }
            }
            // The text takes the collation; it compares by it, as its type
            // does (`fixtures/sql/rowfuncs.json`).
            ExprKind::Collate { expr, ci } => {
                let e = self.node(expr)?;
                let Ty::Str { field, length } = e.ty else {
                    return Err(SqlError::invalid_coercion());
                };
                let field = match (field, ci) {
                    (FieldType::Memo | FieldType::NMemo, _) => field,
                    (_, true) => FieldType::CiCharacter,
                    (FieldType::CiCharacter, false) => FieldType::Character,
                    (field, false) => field,
                };
                let ty = Ty::Str { field, length };
                Node::new(Kind::Cast(Box::new(e), "COLLATE"), ty, pos)
            }
            ExprKind::Cast { expr, to, function } => {
                let e = self.node(expr)?;
                // A memo of bytes is no value to convert
                // (`fixtures/sql/gram_cast.json`).
                if matches!(
                    e.ty,
                    Ty::Bin {
                        field: FieldType::Binary | FieldType::Image,
                        ..
                    }
                ) {
                    return Err(SqlError::bad_argument_why(
                        function,
                        " Argument cannot be LONGVARBINARY.",
                        pos,
                        "",
                    ));
                }
                let mut ty = cast_type(*to, e.ty);
                // The width of the text a cast makes of a cast or a column
                // (`fixtures/sql/castchar2.json`).
                if let (Ty::Str { length, .. }, Some(width)) =
                    (&mut ty, cast_text_width(*to, expr, &e))
                {
                    *length = width;
                }
                // A ROWVERSION is 20 wide as text or bytes
                // (`fixtures/sql/gram_cast.json`).
                // A parameter is text of the width every one is.
                if matches!(e.kind, Kind::Param(_))
                    && let Ty::Str { length, .. } = &mut ty
                    && matches!(
                        to,
                        CastType::Char(None)
                            | CastType::VarChar(None)
                            | CastType::WChar(None)
                            | CastType::WVarChar(None)
                    )
                {
                    *length = PARAM_TEXT;
                }
                // A parameter has no width to give a NUMERIC.
                if matches!(to, CastType::Numeric(None)) && matches!(e.kind, Kind::Param(_)) {
                    return Err(SqlError::bad_argument_why(
                        function,
                        " - must specify both precision and scale.",
                        pos,
                        "",
                    ));
                }
                if let Kind::Column(i) = e.kind
                    && self.is_rowversion(i)
                {
                    match &mut ty {
                        Ty::Str { length, .. } | Ty::Bin { length, .. }
                            if matches!(
                                to,
                                CastType::Char(None)
                                    | CastType::VarChar(None)
                                    | CastType::WChar(None)
                                    | CastType::WVarChar(None)
                                    | CastType::Binary(None)
                                    | CastType::VarBinary(None)
                            ) =>
                        {
                            *length = 20;
                        }
                        _ => {}
                    }
                }
                Node::new(Kind::Cast(Box::new(e), function), ty, pos)
            }
            ExprKind::Extract { part, expr } => {
                let e = self.node(expr)?;
                let func = match part.as_str() {
                    "YEAR" => Func::Year,
                    "MONTH" => Func::Month,
                    "DAY" => Func::Day,
                    "HOUR" => Func::Hour,
                    "MINUTE" => Func::Minute,
                    "SECOND" => Func::Second,
                    "FRAC_SECOND" => Func::FracSecond,
                    "DAYOFWEEK" => Func::DayOfWeek,
                    "DAYOFYEAR" => Func::DayOfYear,
                    "QUARTER" => Func::Quarter,
                    _ => return Err(SqlError::bad_argument("EXTRACT", pos)),
                };
                if !matches!(e.ty, Ty::Date | Ty::Time | Ty::TimeStamp) {
                    return Err(SqlError::bad_argument("EXTRACT", pos));
                }
                Node::new(Kind::Func(func, vec![e]), Ty::SHORT, pos)
            }
            ExprKind::Trim { side, chars, expr } => {
                let e = self.node(expr)?;
                let chars = match chars {
                    Some(c) => Some(Box::new(self.node(c)?)),
                    None => None,
                };
                if !e.ty.is_text() || chars.as_ref().is_some_and(|c| !c.ty.is_text()) {
                    return Err(SqlError::bad_argument("TRIM", pos));
                }
                // Trimmed case-insensitive text stays case-insensitive (a
                // CICharacter padded to its width).
                let ty = if e.ty.case_insensitive() {
                    Ty::Str {
                        field: FieldType::CiCharacter,
                        length: e.ty.length(),
                    }
                } else if e.ty.is_national() {
                    Ty::Str {
                        field: FieldType::NVarChar,
                        length: e.ty.length(),
                    }
                } else {
                    Ty::varchar(e.ty.length())
                };
                Node::new(Kind::Trim(*side, chars, Box::new(e)), ty, pos)
            }
            ExprKind::Position(needle, haystack) => {
                let needle = self.node(needle)?;
                let haystack = self.node(haystack)?;
                if !needle.ty.is_text() || !haystack.ty.is_text() {
                    return Err(SqlError::bad_argument("POSITION", pos));
                }
                Node::new(
                    Kind::Func(Func::Locate, vec![needle, haystack]),
                    Ty::INTEGER,
                    pos,
                )
            }
            ExprKind::Contains {
                all,
                field,
                condition,
            } => {
                let search = self.fts_search(*all, field.as_deref(), condition, "CONTAINS", pos)?;
                Node::new(Kind::Contains(Box::new(search)), Ty::Bool, pos)
            }
            ExprKind::Score { distinct, spec } => {
                let name = if *distinct { "SCOREDISTINCT" } else { "SCORE" };
                let search = match spec {
                    ScoreSpec::Field {
                        all,
                        field,
                        condition,
                    } => self.fts_search(*all, field.as_deref(), condition, name, pos)?,
                    ScoreSpec::Ordinal(n) => {
                        let index = usize::try_from(*n).ok().filter(|&n| n >= 1);
                        let contains = index
                            .and_then(|n| self.where_contains.get(n - 1))
                            .ok_or_else(|| SqlError::bad_argument(name, pos))?;
                        let ExprKind::Contains {
                            all,
                            field,
                            condition,
                        } = &contains.kind
                        else {
                            return Err(SqlError::bad_argument(name, pos));
                        };
                        self.fts_search(*all, field.as_deref(), condition, name, pos)?
                    }
                };
                Node::new(Kind::Score(Box::new(search), *distinct), Ty::INTEGER, pos)
            }
        })
    }

    /// Compiles a `CONTAINS`/`SCORE` over `field` (none for `*`) and `condition`:
    /// resolves the full-text field(s) and the rules to tokenize and parse with,
    /// and pre-parses a constant condition. `name` names the function in errors.
    fn fts_search(
        &mut self,
        all: bool,
        field: Option<&Expr>,
        condition: &Expr,
        name: &str,
        pos: Pos,
    ) -> Result<FtsSearch, SqlError> {
        let (source, rules, indexed) = if all {
            // Every full-text field of the row, each with its own rules.
            let mut fields = Vec::new();
            for (t, scope) in self.tables.iter().enumerate() {
                let base = self.offset(t);
                for (i, field_rules) in scope.fts.iter().enumerate() {
                    if let Some(rules) = field_rules {
                        let ty = Ty::of_field(&scope.fields[i]);
                        let node = Node::new(Kind::Column(base + i), ty, pos);
                        fields.push((node, Arc::new(rules.clone())));
                    }
                }
            }
            // `CONTAINS(*, …)` with no full-text field at all is 7102.
            let Some((_, first)) = fields.first().cloned() else {
                return Err(SqlError::fts(7102));
            };
            (FtsSource::Fields(fields), first, true)
        } else {
            let node = self.node(field.expect("a field for a non-* CONTAINS"))?;
            if !node.ty.is_text() {
                return Err(SqlError::bad_argument(name, pos));
            }
            // A plain field with a full-text index tokenizes with its own rules;
            // anything else (an unindexed field, an expression) with the default.
            let indexed_rules = match &node.kind {
                Kind::Column(i) => self.column_fts(*i),
                _ => None,
            };
            let indexed = indexed_rules.is_some();
            let rules = Arc::new(
                indexed_rules
                    .cloned()
                    .unwrap_or_else(fts::Rules::non_indexed),
            );
            (FtsSource::Expr(Box::new(node)), rules, indexed)
        };

        let mut condition_src = String::new();
        let condition = {
            let node = self.node(condition)?;
            match &node.kind {
                Kind::Const(V::Null) => FtsCondition::Null,
                Kind::Const(V::Str(text)) => match fts::parse(text, &rules) {
                    Ok(query) => {
                        condition_src = text.clone();
                        FtsCondition::Const(Arc::new(query))
                    }
                    Err(error) if self.agg_context == AggContext::Where => {
                        condition_src = text.clone();
                        FtsCondition::Invalid(error.0)
                    }
                    Err(error) => return Err(SqlError::fts(error.0)),
                },
                _ if node.ty.is_text() || node.ty == Ty::Unknown => {
                    FtsCondition::Dynamic(Box::new(node))
                }
                // A non-text, non-NULL condition (a number) is a bad argument.
                _ => return Err(SqlError::bad_argument(name, pos)),
            }
        };
        let filter = self.agg_context == AggContext::Where;
        Ok(FtsSearch {
            source,
            rules,
            indexed,
            filter,
            answered: false,
            condition,
            condition_src,
        })
    }

    /// The full-text rules of the column at row place `global`, if its field has
    /// a full-text index.
    fn column_fts(&self, global: usize) -> Option<&fts::Rules> {
        let mut remaining = global;
        for scope in &self.tables {
            let width = scope.fields.len();
            if remaining < width {
                return scope.fts_rules(remaining);
            }
            remaining -= width;
        }
        None
    }

    /// The subquery `select`, which the compiler must have been told about.
    fn subquery(&self, select: &Select) -> Result<SubInfo, SqlError> {
        self.subqueries
            .get(&(select as *const Select))
            .cloned()
            .ok_or_else(|| SqlError::unsupported("subqueries here", 0))
    }

    fn aggregate(
        &mut self,
        func: Aggregate,
        arg: Option<&Expr>,
        distinct: bool,
        separator: Option<&str>,
        order: &[OrderItem],
        pos: Pos,
    ) -> Result<Node, SqlError> {
        match self.agg_context {
            AggContext::Allowed => {}
            AggContext::Where => return Err(SqlError::aggregate_not_allowed("WHERE or JOIN")),
            AggContext::GroupBy => return Err(SqlError::aggregate_not_allowed("GROUP BY")),
            AggContext::Statement => {
                return Err(SqlError::aggregate_not_allowed("Not a SELECT statement."));
            }
        }
        if let Some(outer) = self.in_aggregate {
            return Err(SqlError::nested_aggregate(outer.name()));
        }
        self.in_aggregate = Some(func);
        let compiled = (|| {
            let arg = arg.map(|a| self.compile(a)).transpose()?;
            // GROUP_CONCAT's ORDER BY keys are per-row expressions, resolved
            // against the same columns as the argument.
            let mut keys = Vec::with_capacity(order.len());
            for item in order {
                let OrderKey::Expr(e) = &item.key else {
                    return Err(SqlError::invalid_aggregate(func.name()));
                };
                keys.push((self.compile(e)?, item.descending));
            }
            Ok((arg, keys))
        })();
        self.in_aggregate = None;
        let (arg, keys) = compiled?;
        let invalid = || SqlError::invalid_aggregate(func.name());
        let ty = match (func, arg.as_ref().map(|a| a.ty)) {
            (Aggregate::Count, Some(ty)) if distinct && ty.is_memo() => return Err(invalid()),
            (Aggregate::Count, _) => Ty::INTEGER,
            (_, None) => return Err(invalid()),
            // GROUP_CONCAT joins fixed- or variable-width text into a
            // VarChar(1024), or of case-insensitive text into a
            // CICharacter(1024) padded to its width; a memo argument is
            // rejected, like a numeric one.
            (Aggregate::GroupConcat, Some(ty)) if ty.is_text() && !ty.is_memo() => {
                if ty.case_insensitive() {
                    Ty::Str {
                        field: FieldType::CiCharacter,
                        length: 1024,
                    }
                } else {
                    Ty::varchar(1024)
                }
            }
            (Aggregate::Sum, Some(Ty::Num { precision, scale })) => Ty::Num {
                precision: precision + 10,
                scale,
            },
            (Aggregate::Sum | Aggregate::Avg, Some(ty)) if ty.is_numeric() => ty,
            (Aggregate::Min | Aggregate::Max, Some(ty))
                if !ty.is_memo() && !matches!(ty, Ty::Guid | Ty::Bin { .. }) =>
            {
                ty
            }
            _ => return Err(invalid()),
        };
        // The same aggregate twice is computed once.
        let spec = AggSpec {
            func,
            arg,
            distinct,
            ty,
            separator: separator.map(str::to_owned),
            order: keys,
        };
        let same = |a: &AggSpec| {
            let text = |s: &AggSpec| crate::query::without_positions(&format!("{s:?}"));
            text(a) == text(&spec)
        };
        let index = match self.aggregates.iter().position(same) {
            Some(index) => index,
            None => {
                self.aggregates.push(spec);
                self.aggregates.len() - 1
            }
        };
        Ok(Node::new(Kind::Agg(index), ty, pos))
    }

    fn binary(
        &mut self,
        op: BinOp,
        a: &Expr,
        b: &Expr,
        pos: Pos,
        cond: bool,
    ) -> Result<Node, SqlError> {
        let cond = cond && matches!(op, BinOp::And | BinOp::Or);
        // An operand that is a qualified column is pointed at after its
        // qualifier.
        let operand_at = match &a.kind {
            ExprKind::Column {
                table: Some(qualifier),
                ..
            } => a.pos + qualifier.chars().count() + 1,
            _ => pos,
        };
        self.cond = cond;
        let mut a = self.node(a)?;
        self.cond = cond;
        let mut b = self.node(b)?;
        // `CONTAINS(…) = TRUE` is the `CONTAINS`, as the original engine's
        // plans show it, and `= FALSE` its `NOT`, which its full-text order
        // answers (`fixtures/sql/fts_plans.json`).
        if op == BinOp::Eq && matches!(a.kind, Kind::Contains(_)) {
            match b.kind {
                Kind::Const(V::Bool(true)) => return Ok(a),
                Kind::Const(V::Bool(false)) => {
                    return Ok(Node::new(Kind::Not(Box::new(a)), Ty::Bool, pos));
                }
                _ => {}
            }
        }
        // A parameter as an operand of `AND` or `OR` is typed by nothing,
        // the right one reported first (`fixtures/sql/params2.json`).
        if matches!(op, BinOp::And | BinOp::Or) {
            for operand in [&b, &a] {
                if matches!(operand.kind, Kind::Param(_)) {
                    return Err(SqlError::parameter_type(operand.pos));
                }
            }
        }
        let untyped = |n: &Node| n.ty == Ty::Unknown && matches!(n.kind, Kind::Param(_));
        if untyped(&a) && untyped(&b) {
            return Err(SqlError::null_type());
        }
        // A parameter joined to text is text of the width every parameter of
        // text has.
        if op == BinOp::Add {
            if b.ty.is_text() {
                self.settle(&mut a, Ty::varchar(PARAM_TEXT));
            }
            if a.ty.is_text() {
                self.settle(&mut b, Ty::varchar(PARAM_TEXT));
            }
        }
        // A parameter added to a date is a number of days.
        if op == BinOp::Add {
            let days = Ty::INTEGER;
            if a.ty == Ty::Date {
                self.settle(&mut b, days);
            }
            if b.ty == Ty::Date {
                self.settle(&mut a, days);
            }
        }
        self.settle(&mut a, b.ty);
        self.settle(&mut b, a.ty);
        // Two texts given a collation each must have the same
        // (`fixtures/sql/rowfuncs.json`).
        let explicit = |n: &Node| matches!(&n.kind, Kind::Cast(_, "COLLATE"));
        if explicit(&a) && explicit(&b) && a.ty.case_insensitive() != b.ty.case_insensitive() {
            return Err(SqlError::collation_mismatch());
        }
        let invalid = || SqlError::invalid_operand(op.symbol(), operand_at);
        let ty = match op {
            BinOp::And | BinOp::Or => {
                a = truth(a);
                b = truth(b);
                Ty::Bool
            }
            _ if op.is_comparison() => {
                if b.kind_is_column() {
                    check_collations(&a, b.ty)?;
                }
                // A logical value is compared with another, or with the
                // constants 0 and 1 (`fixtures/sql/claims.json`).
                if let (Ty::Bool, other, node) | (other, Ty::Bool, node) = (a.ty, b.ty, &b)
                    && other.is_numeric()
                {
                    let other_node = if a.ty == Ty::Bool { node } else { &a };
                    let bit = matches!(other_node.constant(), Some(V::Int(0 | 1)));
                    if !bit {
                        return Err(SqlError::invalid_operand_because(
                            op.symbol(),
                            "Boolean value cannot be operated with non-Boolean value.",
                            operand_at,
                        ));
                    }
                }
                if !comparable(a.ty, b.ty) {
                    return Err(invalid());
                }
                Ty::Bool
            }
            BinOp::BitOr | BinOp::BitAnd | BinOp::BitXor => match (a.ty, b.ty) {
                _ if doubled(&a) || doubled(&b) => {
                    let at = if doubled(&a) { a.pos } else { b.pos };
                    return Err(SqlError::invalid_operand_because(
                        op.symbol(),
                        "Double value cannot be an operand of a bitwise operator.",
                        at,
                    ));
                }
                (Ty::Double | Ty::CurDouble, _) | (_, Ty::Double | Ty::CurDouble) => {
                    let at = if matches!(a.ty, Ty::Double | Ty::CurDouble) {
                        a.pos
                    } else {
                        b.pos
                    };
                    return Err(SqlError::invalid_operand_because(
                        op.symbol(),
                        "Double value cannot be an operand of a bitwise operator.",
                        at,
                    ));
                }
                (Ty::Money, _) | (_, Ty::Money) => {
                    let at = if a.ty == Ty::Money { a.pos } else { b.pos };
                    return Err(SqlError::invalid_operand_because(
                        op.symbol(),
                        "Money value cannot be an operand of a bitwise operator.",
                        at,
                    ));
                }
                (Ty::Num { precision, scale }, other) | (other, Ty::Num { precision, scale })
                    if (scale > 0 || precision > 18)
                        && matches!(other, Ty::Int { .. } | Ty::Num { .. }) =>
                {
                    return Err(SqlError::invalid_operand_because(
                        op.symbol(),
                        "Numeric operands of a bitwise operator cannot have decimal or \
                         precision greater than 18.",
                        pos,
                    ));
                }
                (Ty::Bool, Ty::Bool) => Ty::Bool,
                (Ty::Bin { .. }, Ty::Int { .. }) | (Ty::Int { .. }, Ty::Bin { .. }) => Ty::INTEGER,
                (Ty::Int { width: wa, .. }, Ty::Int { width: wb, .. }) => {
                    let width = wa.max(wb);
                    Ty::Int {
                        width,
                        digits: match width {
                            IntWidth::Long => 19,
                            IntWidth::Int => 10,
                            IntWidth::Short => 5,
                        },
                    }
                }
                (Ty::Bin { .. }, Ty::Bin { .. }) => {
                    return Err(SqlError::invalid_operand_because(
                        op.symbol(),
                        "Only one operand of a bitwise operator can be binary.",
                        pos,
                    ));
                }
                _ => return Err(invalid()),
            },
            BinOp::Shl | BinOp::Shr => shift_type(op, &a, &b, pos)?,
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => match (a.ty, b.ty) {
                (
                    Ty::Str {
                        field: fa,
                        length: la,
                    },
                    Ty::Str {
                        field: fb,
                        length: lb,
                    },
                ) if op == BinOp::Add => {
                    if mixed_collations(&a, &b) {
                        return Err(SqlError::ambiguous_character());
                    }
                    let memo = |f| matches!(f, FieldType::Memo | FieldType::NMemo);
                    let wide = |f| matches!(f, FieldType::NChar | FieldType::NVarChar);
                    let fixed = |f| matches!(f, FieldType::Character | FieldType::NChar);
                    let field = if fa == FieldType::NMemo
                        || fb == FieldType::NMemo
                        || (memo(fa) && wide(fb))
                        || (memo(fb) && wide(fa))
                    {
                        FieldType::NMemo
                    } else if memo(fa) || memo(fb) {
                        FieldType::Memo
                    } else if wide(fa) || wide(fb) {
                        // Unicode text joined stays fixed-length only with
                        // fixed-length text (`fixtures/sql/selinto.json`).
                        match fixed(fa) && fixed(fb) {
                            true => FieldType::NChar,
                            false => FieldType::NVarChar,
                        }
                    } else if fa == FieldType::CiCharacter || fb == FieldType::CiCharacter {
                        FieldType::CiCharacter
                    } else if fa == FieldType::Character && fb == FieldType::Character {
                        // Fixed-length text joined stays fixed-length
                        // (`fixtures/sql/spview.json`).
                        FieldType::Character
                    } else {
                        FieldType::VarChar
                    };
                    // Unicode text stops at 32500 code units: variable-length
                    // text is cut there, and two fixed-length texts refuse to
                    // be joined (`fixtures/sql/claims.json`).
                    let mut length = la.saturating_add(lb);
                    // Joined text that cannot be kept in the width of a
                    // character field is a memo.
                    let field = match !wide(field) && !memo(field) && length >= 65530 {
                        true => FieldType::Memo,
                        false => field,
                    };
                    if wide(field) && length > 32500 {
                        if field == FieldType::NChar {
                            return Err(SqlError::internal_error());
                        }
                        length = 32500;
                    }
                    Ty::Str { field, length }
                }
                (Ty::Date, t)
                    if (t.exact().is_some() || t == Ty::Double)
                        && matches!(op, BinOp::Add | BinOp::Sub) =>
                {
                    Ty::Date
                }
                (t, Ty::Date) if (t.exact().is_some() || t == Ty::Double) && op == BinOp::Add => {
                    Ty::Date
                }
                // A number less a date is a failure of the engine's own.
                (t, Ty::Date) if t.exact().is_some() && op == BinOp::Sub => {
                    return Err(SqlError::internal_error());
                }
                (Ty::Date, Ty::Date) if op == BinOp::Sub => Ty::INTEGER,
                (x, y)
                    if op == BinOp::Mul
                        && x.exact()
                            .zip(y.exact())
                            .is_some_and(|((_, a), (_, b))| a + b > 30) =>
                {
                    return Err(SqlError::numeric_scale(pos));
                }
                (x, y) => types::arithmetic(op, x, y).ok_or_else(|| {
                    // A text constant that is no date, time or timestamp
                    // says so (`fixtures/sql/tsarith2.json`).
                    [(&a, &b), (&b, &a)]
                        .into_iter()
                        .find_map(|(other, text)| match &text.kind {
                            Kind::Const(v) if text.ty.is_text() => crate::eval::unreadable_text(
                                v,
                                text.ty,
                                other.ty,
                                &crate::eval::Env::default(),
                            ),
                            _ => None,
                        })
                        .map_or_else(invalid, |what| {
                            SqlError::invalid_operand_value(op.symbol(), what, pos)
                        })
                })?,
            },
            _ => return Err(invalid()),
        };
        Ok(Node::new(
            Kind::Binary(op, Box::new(a), Box::new(b)),
            ty,
            pos,
        ))
    }

    fn case(
        &mut self,
        operand: Option<&Expr>,
        whens: &[(Expr, Expr)],
        otherwise: Option<&Expr>,
        pos: Pos,
    ) -> Result<Node, SqlError> {
        // `CASE` types no parameter: the operand and the `ELSE` are looked
        // at first, then each `WHEN` and `THEN` in turn, and the error names
        // the number of the pair (`fixtures/sql/params2.json`).
        let param = |e: &Expr| matches!(e.kind, ExprKind::Param(_));
        if operand.is_some_and(param) || otherwise.is_some_and(param) {
            return Err(SqlError::case_parameter(None));
        }
        if let Some(i) = whens.iter().position(|(w, t)| param(w) || param(t)) {
            return Err(SqlError::case_parameter(Some(i + 1)));
        }
        let operand = match operand {
            Some(e) => Some(Box::new(self.node(e)?)),
            None => None,
        };
        let mut ty = Ty::Unknown;
        let mut nodes = Vec::with_capacity(whens.len());
        for (index, (when, then)) in whens.iter().enumerate() {
            let mut when = self.node(when)?;
            match &operand {
                Some(operand) => {
                    self.settle(&mut when, operand.ty);
                    if !comparable(operand.ty, when.ty) {
                        return Err(SqlError::case_when_type(index + 1));
                    }
                }
                None if when.ty != Ty::Bool => return Err(SqlError::case_not_boolean(when.pos)),
                None => {}
            }
            let then = self.node(then)?;
            if let Some(previous) = nodes.last().map(|(_, n): &(Node, Node)| n)
                && mixed_collations(previous, &then)
            {
                return Err(SqlError::ambiguous_character());
            }
            ty = types::merge(ty, then.ty).ok_or_else(|| {
                match (ty.is_temporal(), then.ty.is_temporal()) {
                    (true, true) => SqlError::branch_types("CASE", Some(index + 1), then.pos),
                    _ => SqlError::invalid_operand("CASE", then.pos),
                }
            })?;
            nodes.push((when, then));
        }
        let otherwise = match otherwise {
            Some(e) => {
                let e = self.node(e)?;
                if let Some((_, previous)) = nodes.last()
                    && mixed_collations(previous, &e)
                {
                    return Err(SqlError::ambiguous_character());
                }
                ty = types::merge(ty, e.ty).ok_or_else(|| {
                    match (ty.is_temporal(), e.ty.is_temporal()) {
                        // The original numbers the `ELSE` 1 whatever the
                        // `WHEN`s before it (`fixtures/sql/claims.json`).
                        (true, true) => SqlError::branch_types("CASE", Some(1), e.pos),
                        _ => SqlError::invalid_operand("CASE", e.pos),
                    }
                })?;
                Some(Box::new(e))
            }
            None => None,
        };
        if ty == Ty::Unknown {
            return Err(SqlError::case_untyped());
        }
        Ok(Node::new(Kind::Case(operand, nodes, otherwise), ty, pos))
    }
}

thread_local! {
    /// Whether an `ORDER BY` key is being compiled, where texts of two
    /// collations do join (the key is sorted, not shown).
    static ORDERING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` as the compilation of an `ORDER BY` key.
pub(crate) fn ordering<T>(f: impl FnOnce() -> T) -> T {
    let before = ORDERING.replace(true);
    let result = f();
    ORDERING.set(before);
    result
}

/// Whether two texts of different collations are joined or merged: the
/// original does not choose one for them (of columns; a constant, a
/// variable or an expression takes the other's).
pub(crate) fn mixed_collations(a: &Node, b: &Node) -> bool {
    if ORDERING.get() {
        return false;
    }
    let explicit = |n: &Node| matches!(&n.kind, Kind::Cast(_, "COLLATE"));
    !explicit(a)
        && !explicit(b)
        && a.ty.is_text()
        && b.ty.is_text()
        && a.ty.case_insensitive() != b.ty.case_insensitive()
        && a.kind_is_column()
        && b.kind_is_column()
}

impl Node {
    /// Whether the node reads a column (of this row or an outer one).
    fn kind_is_column(&self) -> bool {
        matches!(self.kind, Kind::Column(_) | Kind::Outer(..))
    }
}

/// Whether the original engine takes `node` for a double where a bitwise
/// operator is concerned, though its type is an integer: `ABS`, `CEILING`,
/// `FLOOR`, `ROUND` and `TRUNCATE` of one, and what is worked out from it
/// (`fixtures/sql/gram_cast.json`).
fn doubled(node: &Node) -> bool {
    match &node.kind {
        Kind::Func(Func::Abs | Func::Ceiling | Func::Floor | Func::Round | Func::Truncate, _) => {
            matches!(node.ty, Ty::Int { .. })
        }
        Kind::Neg(inner) => doubled(inner),
        Kind::Binary(BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div, a, b) => {
            doubled(a) || doubled(b)
        }
        _ => false,
    }
}

/// The result type of `<<` and `>>`, which is the left operand's: an integer
/// or an exact number of no decimals (of at most 18 digits), or a logical
/// value (`fixtures/sql/gram_shift.json`).
fn shift_type(op: BinOp, a: &Node, b: &Node, pos: Pos) -> Result<Ty, SqlError> {
    let refuse = |why: &str, at: Pos| SqlError::invalid_operand_because(op.symbol(), why, at);
    // Where the original puts the error: at the operand for a double, money
    // or a logical value on the right, at the expression for the rest.
    let integral = |n: &Node| -> Result<Option<Ty>, SqlError> {
        Ok(match n.ty {
            Ty::Int { .. } => Some(n.ty),
            Ty::Unknown => Some(Ty::INTEGER),
            Ty::Num { precision, scale } if scale == 0 && precision <= 18 => {
                Some(match precision {
                    0..=4 => Ty::SHORT,
                    5..=9 => Ty::INTEGER,
                    _ => Ty::LONG,
                })
            }
            Ty::Num { .. } => {
                return Err(refuse(
                    "Numeric operands of a bitwise operator cannot have decimal or precision \
                     greater than 18.",
                    pos,
                ));
            }
            Ty::Double | Ty::CurDouble => {
                return Err(refuse(
                    "Double value cannot be an operand of a bitwise operator.",
                    n.pos,
                ));
            }
            Ty::Money => {
                return Err(refuse(
                    "Money value cannot be an operand of a bitwise operator.",
                    n.pos,
                ));
            }
            Ty::Bin { .. } => {
                return Err(refuse(
                    "The bitwise operator does not support binary operands.",
                    pos,
                ));
            }
            Ty::Bool => None,
            _ => return Err(SqlError::invalid_operand(op.symbol(), pos)),
        })
    };
    // The functions that round a number make a double of it for the
    // operators, though the result is shown as an integer.
    for n in [a, b] {
        if doubled(n) {
            return Err(refuse(
                "Double value cannot be an operand of a bitwise operator.",
                n.pos,
            ));
        }
    }
    let left = integral(a)?;
    let right = integral(b)?;
    match (left, right) {
        (Some(ty), Some(_)) => Ok(ty),
        (None, _) => Ok(Ty::Bool),
        (Some(_), None) => Err(refuse(
            "Boolean value cannot be the right operand of a bitwise SHIFT operator.",
            b.pos,
        )),
    }
}

/// 2213 when a column compares with a column (or a subquery's) of another
/// collation: text that ignores case with text that does not.
fn check_collations(a: &Node, b: Ty) -> Result<(), SqlError> {
    let ci = |ty: Ty| {
        matches!(
            ty,
            Ty::Str {
                field: FieldType::CiCharacter,
                ..
            }
        )
    };
    let cs = |ty: Ty| {
        matches!(
            ty,
            Ty::Str {
                field: FieldType::Character | FieldType::VarChar,
                ..
            }
        )
    };
    if a.kind_is_column() && ((ci(a.ty) && cs(b)) || (cs(a.ty) && ci(b))) {
        return Err(SqlError::collation_mismatch());
    }
    Ok(())
}

/// The name of the column `expr` is, which a collation does not rename.
pub(crate) fn collated_column(expr: &Expr) -> Option<String> {
    match &expr.kind {
        ExprKind::Column { name, .. } => Some(name.clone()),
        _ => None,
    }
}

/// Whether values of `a` and `b` can be compared.
pub(crate) fn comparable(a: Ty, b: Ty) -> bool {
    match (a, b) {
        (Ty::Unknown, _) | (_, Ty::Unknown) => true,
        (x, y) if x.is_numeric() && y.is_numeric() => true,
        (Ty::Bool, y) | (y, Ty::Bool) => y == Ty::Bool || y.is_numeric(),
        (Ty::Str { .. }, Ty::Str { .. }) | (Ty::Bin { .. }, Ty::Bin { .. }) => true,
        (Ty::Date, Ty::Date | Ty::Str { .. }) | (Ty::Str { .. }, Ty::Date) => true,
        (Ty::Time, Ty::Time | Ty::Str { .. }) | (Ty::Str { .. }, Ty::Time) => true,
        (Ty::TimeStamp, Ty::TimeStamp | Ty::Str { .. }) | (Ty::Str { .. }, Ty::TimeStamp) => true,
        (Ty::Guid, Ty::Guid | Ty::Str { .. }) | (Ty::Str { .. }, Ty::Guid) => true,
        (Ty::Guid, Ty::Bin { .. }) | (Ty::Bin { .. }, Ty::Guid) => true,
        _ => false,
    }
}

/// The width of a parameter's text, where nothing else gives it one: the
/// original engine's own (`fixtures/sql/params.json`).
pub(crate) const PARAM_TEXT: u16 = 1024;

pub(crate) fn clamp(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// The `NEWIDSTRING` format a SQL bareword names, as the letter
/// [`tflip_expr::guid_string`] takes; `None` for one the SQL engine rejects
/// (2159). `MIME` maps to `M` — the SQL scalar accepts it, where the
/// expression engine does not (probed on the VM; see `docs/ADS_BUGS.md`).
fn sql_guid_format(word: &str) -> Option<&'static str> {
    Some(match word.to_ascii_uppercase().as_str() {
        "D" | "DELIMITED" => "D",
        "B" | "BRACKETED" | "CURLYBRACES" => "B",
        "P" | "PARENTHESIS" => "P",
        "N" | "NUMBERS" => "N",
        "M" | "MIME" => "M",
        "F" | "FILE" => "F",
        _ => return None,
    })
}

/// A number literal: an INTEGER if it fits, else a BIGINT, else a NUMERIC;
/// with a point, a NUMERIC of its digits; with an exponent, a DOUBLE. Its
/// type is decided before its sign.
fn number(text: &str, pos: Pos) -> Result<Node, SqlError> {
    let unsigned = text.trim_start_matches('-');
    let negative = (text.len() - unsigned.len()) % 2 == 1;
    let invalid = || SqlError::expected("identifier or expression", pos, "");
    if unsigned.contains(['e', 'E']) {
        let value = exponent_number(unsigned).ok_or_else(invalid)?;
        return Ok(Node::new(
            Kind::Const(V::Double(if negative { -value } else { value })),
            Ty::Double,
            pos,
        ));
    }
    // A point with no decimals after it is a whole number.
    let unsigned = match unsigned.strip_suffix('.') {
        Some(whole) if !whole.contains(['.', 'e', 'E']) && !whole.is_empty() => whole,
        _ => unsigned,
    };
    let digits = clamp(unsigned.bytes().filter(u8::is_ascii_digit).count());
    // More digits than a NUMERIC holds make a DOUBLE.
    if digits > 30 {
        let value = exponent_number(&format!("{unsigned}e0")).ok_or_else(invalid)?;
        return Ok(Node::new(
            Kind::Const(V::Double(if negative { -value } else { value })),
            Ty::Double,
            pos,
        ));
    }
    if let Some((_, fraction)) = unsigned.split_once('.') {
        let scale = clamp(fraction.len());
        let n = crate::value::parse_exact(unsigned, scale).ok_or_else(invalid)?;
        let ty = Ty::Num {
            precision: digits.max(scale).max(1),
            scale,
        };
        return Ok(Node::new(
            Kind::Const(V::Num(if negative { -n } else { n }, scale)),
            ty,
            pos,
        ));
    }
    let n: i128 = unsigned.parse().map_err(|_| invalid())?;
    let signed = if negative { -n } else { n };
    let (ty, value) = if i32::try_from(n).is_ok() {
        (
            Ty::Int {
                width: IntWidth::Int,
                digits,
            },
            V::Int(signed as i64),
        )
    } else if i64::try_from(n).is_ok() {
        (
            Ty::Int {
                width: IntWidth::Long,
                digits,
            },
            V::Int(signed as i64),
        )
    } else {
        (
            Ty::Num {
                precision: digits,
                scale: 0,
            },
            V::Num(signed, 0),
        )
    };
    Ok(Node::new(Kind::Const(value), ty, pos))
}

/// A number with an exponent (`1.5e30`), as the original engine reads one
/// (probed with `tools/ace/sql/sql.c`): its digits gathered in a double (each
/// time ×10 plus the digit), then multiplied by 10, or divided, once for each
/// power that the exponent less the fraction's digits leaves, each step
/// rounded. So the value can be a few units off in its 17th digit: `1e30` is
/// 9.9999999999999988e29, `123.456e0` 123.45599999999999.
fn exponent_number(text: &str) -> Option<f64> {
    let (mantissa, exponent) = text.split_once(['e', 'E'])?;
    let exponent: i64 = exponent.parse().ok()?;
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = whole.bytes().chain(fraction.bytes());
    if whole.len() + fraction.len() == 0 || !digits.clone().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut value = digits.fold(0.0, |n, d| n * 10.0 + f64::from(d - b'0'));
    let power = exponent.saturating_sub(fraction.len() as i64);
    for _ in 0..power.unsigned_abs() {
        if value == 0.0 || value.is_infinite() {
            break;
        }
        value = if power > 0 {
            value * 10.0
        } else {
            value / 10.0
        };
    }
    Some(value)
}

/// `{d '2026-09-29'}`, `{t '13:45:30'}`, `{ts '2026-09-29 13:45:30.123'}`.
fn typed_literal(literal: TypedLiteral, text: &str, pos: Pos) -> Result<Node, SqlError> {
    let bad = || SqlError::conversion("character string to date", "ESCAPE", pos);
    let (value, ty) = match literal {
        TypedLiteral::Date => (V::Date(iso_date(text).ok_or_else(bad)?), Ty::Date),
        TypedLiteral::Time => (V::Time(iso_time(text).ok_or_else(bad)?), Ty::Time),
        TypedLiteral::TimeStamp => {
            let (day, ms) = iso_timestamp(text).ok_or_else(bad)?;
            (V::TimeStamp(day, ms), Ty::TimeStamp)
        }
    };
    Ok(Node::new(Kind::Const(value), ty, pos))
}

/// `YYYY-MM-DD` as a Julian day.
pub(crate) fn iso_date(text: &str) -> Option<i32> {
    let mut parts = text.trim().splitn(3, '-');
    let year: i32 = parts.next()?.trim().parse().ok()?;
    let month: u32 = parts.next()?.trim().parse().ok()?;
    let day: u32 = parts.next()?.trim().parse().ok()?;
    tflip_storage::value::ymd_to_julian(tflip_storage::value::Ymd { year, month, day })
}

/// `YYYY-MM-DD` of exactly four, two and two digits as a Julian day (the
/// original engine reads no other ISO form: `tools/ace/sql/literals.txt`).
pub(crate) fn strict_iso_date(text: &str) -> Option<i32> {
    let bytes = text.as_bytes();
    let iso = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| matches!(i, 4 | 7) || b.is_ascii_digit());
    iso.then(|| iso_date(text)).flatten()
}

/// A date as text: ISO, or in `format` with all of the text used.
pub(crate) fn date_text(text: &str, format: &str, epoch: u16) -> Option<i32> {
    let text = text.trim();
    if let Some(julian) = strict_iso_date(text) {
        return Some(julian);
    }
    // The format's own separators only: `12-31-2012` is no date in `MM/DD/CCYY`.
    let separators = |c: char| !c.is_ascii_digit() && !format.contains(c);
    if text.chars().any(separators) {
        return None;
    }
    match tflip_expr::text::parse_date_with(text, format, epoch) {
        Ok(Some((julian, end))) if end == text.chars().count() => Some(julian),
        _ => None,
    }
}

/// `HH:MM` of two digits each, then `:SS` and `.fff` if there: the hour, the
/// milliseconds since the hour, and the text that follows.
fn clock(text: &str, seconds: bool) -> Option<(u32, u32, &str)> {
    let bytes = text.as_bytes();
    let two = |at: usize| match bytes.get(at..at + 2) {
        Some([a, b]) if a.is_ascii_digit() && b.is_ascii_digit() => {
            Some(u32::from(a - b'0') * 10 + u32::from(b - b'0'))
        }
        _ => None,
    };
    let hours = two(0)?;
    let minutes = two(3).filter(|m| bytes[2] == b':' && *m < 60)?;
    let mut ms = minutes * 60_000;
    let mut used = 5;
    if bytes.get(5) == Some(&b':') {
        ms += two(6).filter(|s| *s < 60)? * 1000;
        used = 8;
        if bytes.get(8) == Some(&b'.') {
            used = 9;
            while bytes.get(used).is_some_and(u8::is_ascii_digit) {
                if used < 12 {
                    ms += u32::from(bytes[used] - b'0') * [100, 10, 1][used - 9];
                }
                used += 1;
            }
        }
    } else if seconds {
        return None;
    }
    Some((hours, ms, &text[used..]))
}

/// A time as text: `HH:MM[:SS[.fff]]`, then `AM` or `PM` if it has one (a
/// 12-hour clock, where 12 AM is midnight), as milliseconds since midnight.
pub(crate) fn time_text(text: &str) -> Option<u32> {
    let (hours, ms, rest) = clock(text.trim(), false)?;
    let hours = match rest.trim_start().to_ascii_lowercase().as_str() {
        "" if hours < 24 => hours,
        "am" if hours <= 12 => hours % 12,
        "pm" if hours <= 12 => hours % 12 + 12,
        _ => return None,
    };
    Some(hours * 3_600_000 + ms)
}

/// A timestamp as text: a date, a space or a `T`, and the time with no
/// `AM` or `PM`, which may have a `Z` or an offset after it (ignored).
pub(crate) fn timestamp_text(text: &str, format: &str, epoch: u16) -> Option<(i32, u32)> {
    let text = text.trim();
    let (date, time) = text.split_once([' ', 'T', 't'])?;
    let (hours, ms, rest) = clock(time, false)?;
    let offset = rest.len() == 6
        && matches!(rest.as_bytes()[0], b'+' | b'-')
        && rest[1..3].bytes().all(|b| b.is_ascii_digit())
        && rest.as_bytes()[3] == b':'
        && rest[4..].bytes().all(|b| b.is_ascii_digit());
    if hours > 23 || !(rest.is_empty() || rest.eq_ignore_ascii_case("z") || offset) {
        return None;
    }
    Some((date_text(date, format, epoch)?, hours * 3_600_000 + ms))
}

/// The text of a `DATE`, `TIME` or `TIMESTAMP` literal or escape as the
/// canonical `YYYY-MM-DD`, `HH:MM:SS.mmm` or both, for [`typed_literal`];
/// `None` if it is not one. An escape's time has seconds and no `AM` or
/// `PM`. The date format is the default one: a literal is read before the
/// connection is known.
pub(crate) fn typed_text(literal: TypedLiteral, text: &str, escape: bool) -> Option<String> {
    let (format, epoch) = ("MM/DD/CCYY", tflip_expr::text::DEFAULT_EPOCH);
    let day = |julian: i32| {
        let ymd = tflip_storage::value::julian_to_ymd(julian);
        format!("{:04}-{:02}-{:02}", ymd.year, ymd.month, ymd.day)
    };
    let time = |ms: u32| {
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            ms / 3_600_000,
            ms / 60_000 % 60,
            ms / 1000 % 60,
            ms % 1000
        )
    };
    match literal {
        TypedLiteral::Date => date_text(text, format, epoch).map(day),
        TypedLiteral::Time => {
            if escape && clock(text.trim(), true).is_none_or(|(_, _, rest)| !rest.is_empty()) {
                return None;
            }
            time_text(text).map(time)
        }
        TypedLiteral::TimeStamp => {
            let (julian, ms) = timestamp_text(text, format, epoch)?;
            Some(format!("{} {}", day(julian), time(ms)))
        }
    }
}

/// `HH:MM[:SS[.fff]]` as milliseconds since midnight.
pub(crate) fn iso_time(text: &str) -> Option<u32> {
    let mut parts = text.trim().splitn(3, ':');
    let hours: u32 = parts.next()?.trim().parse().ok()?;
    let minutes: u32 = parts.next()?.trim().parse().ok()?;
    let seconds = parts.next().unwrap_or("0").trim();
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let seconds: u32 = whole.parse().ok()?;
    let mut ms = 0;
    for (i, b) in fraction.bytes().take(3).enumerate() {
        if !b.is_ascii_digit() {
            return None;
        }
        ms += u32::from(b - b'0') * [100, 10, 1][i];
    }
    if hours > 23 || minutes > 59 || seconds > 59 {
        return None;
    }
    Some(((hours * 60 + minutes) * 60 + seconds) * 1000 + ms)
}

/// `YYYY-MM-DD[ HH:MM:SS[.fff]]`.
pub(crate) fn iso_timestamp(text: &str) -> Option<(i32, u32)> {
    let text = text.trim();
    match text.split_once([' ', 'T']) {
        Some((date, time)) => Some((iso_date(date)?, iso_time(time)?)),
        None => Some((iso_date(text)?, 0)),
    }
}

/// The type `CAST(… AS to)` gives a value of type `from`.
/// How wide the text of `CAST(expr AS to)` is, where `to` has no length and
/// `e`, the compiled `expr`, has a type the original widens: a `TINYINT` cast
/// is 4, a `REAL` 13 and a `BIGINT` cast of a number as its digits and one
/// more, and a `MONEY` column 24.
fn cast_text_width(to: CastType, expr: &Expr, e: &Node) -> Option<u16> {
    if !matches!(
        to,
        CastType::Char(None)
            | CastType::VarChar(None)
            | CastType::CiChar(None)
            | CastType::WChar(None)
            | CastType::WVarChar(None)
    ) {
        return None;
    }
    match (&expr.kind, &e.kind) {
        (
            ExprKind::Cast {
                to: CastType::TinyInt,
                ..
            },
            _,
        ) => Some(4),
        (
            ExprKind::Cast {
                to: CastType::Real, ..
            },
            _,
        ) => Some(13),
        (
            ExprKind::Cast {
                to: CastType::BigInt,
                ..
            },
            Kind::Cast(inner, _),
        ) => match &inner.kind {
            Kind::Const(V::Int(n)) => Some(n.unsigned_abs().to_string().len() as u16 + 1),
            _ => None,
        },
        (_, Kind::Column(_)) if e.ty == Ty::Money => Some(24),
        _ => None,
    }
}

/// How many bytes a cast without a length makes of a value of the type
/// `from`: its own bytes or characters, a number's width.
fn binary_length(from: Ty) -> u16 {
    match from {
        Ty::Int {
            width: IntWidth::Short,
            ..
        } => 2,
        Ty::Int {
            width: IntWidth::Long,
            ..
        } => 8,
        Ty::Int { .. } => 4,
        Ty::Double | Ty::CurDouble => 8,
        Ty::Bin { length, .. } | Ty::Str { length, .. } => length,
        Ty::Guid => 16,
        _ => 1,
    }
}

fn cast_type(to: CastType, from: Ty) -> Ty {
    let text_length = |length: Option<u16>| {
        length.unwrap_or(match from {
            Ty::Str {
                field: FieldType::Memo | FieldType::NMemo,
                ..
            } => 1024,
            Ty::Str { length, .. } => length,
            Ty::Int {
                width: IntWidth::Short,
                ..
            } => 6,
            Ty::Int {
                width: IntWidth::Long,
                ..
            } => 21,
            Ty::Int { .. } => 11,
            // The digits, a sign, and a decimal point if it has any.
            Ty::Num { precision, scale } => precision + 1 + u16::from(scale > 0),
            Ty::Double | Ty::CurDouble => 22,
            Ty::Money => 21,
            Ty::Date => 10,
            Ty::Time => 8,
            Ty::TimeStamp => 23,
            Ty::Bin { length, .. } => length,
            // `CAST(guid AS SQL_CHAR)` is 1024 long (`fixtures/sql/literals.json`),
            // and so is that of a memo field.
            Ty::Guid => 1024,
            _ => 1,
        })
    };
    match to {
        CastType::Char(length) => Ty::Str {
            // Without a length, the text of a `CICHARACTER` stays one.
            field: match (length, from) {
                (
                    None,
                    Ty::Str {
                        field: FieldType::CiCharacter,
                        ..
                    },
                ) => FieldType::CiCharacter,
                _ => FieldType::Character,
            },
            length: text_length(length),
        },
        CastType::CiChar(length) => Ty::Str {
            field: FieldType::CiCharacter,
            length: text_length(length),
        },
        CastType::VarChar(length) => Ty::Str {
            field: FieldType::VarChar,
            length: text_length(length),
        },
        CastType::LongVarChar => Ty::Str {
            field: FieldType::Memo,
            length: 9,
        },
        CastType::WChar(length) => Ty::Str {
            field: FieldType::NChar,
            length: text_length(length),
        },
        CastType::WVarChar(length) => Ty::Str {
            field: FieldType::NVarChar,
            length: text_length(length),
        },
        CastType::WLongVarChar => Ty::Str {
            field: FieldType::NMemo,
            length: 9,
        },
        CastType::Binary(length) => Ty::Bin {
            field: FieldType::Raw,
            length: length.unwrap_or_else(|| binary_length(from)),
        },
        CastType::VarBinary(length) => Ty::Bin {
            field: FieldType::VarBinary,
            length: length.unwrap_or_else(|| binary_length(from)),
        },
        CastType::LongVarBinary => Ty::Bin {
            field: FieldType::Binary,
            length: 9,
        },
        CastType::Bit => Ty::Bool,
        CastType::TinyInt | CastType::SmallInt => Ty::SHORT,
        CastType::Integer => Ty::INTEGER,
        CastType::BigInt => Ty::LONG,
        CastType::Numeric(Some((precision, scale)))
        | CastType::Decimal(Some((precision, scale))) => Ty::Num {
            // A NUMERIC is at most 32 wide.
            precision: precision.min(31 - u16::from(scale > 0)),
            scale,
        },
        // Without a size, the number as wide as the one it is made of
        // (`fixtures/sql/gram_cast3.json`).
        CastType::Numeric(None) | CastType::Decimal(None) => match from {
            Ty::Int {
                width: IntWidth::Long,
                ..
            } => Ty::Num {
                precision: 20,
                scale: 0,
            },
            Ty::Int { digits, .. } => Ty::Num {
                precision: digits,
                scale: 0,
            },
            Ty::Num { .. } => from,
            Ty::Money => Ty::Num {
                precision: 19,
                scale: 4,
            },
            _ => Ty::Num {
                precision: 15,
                scale: 3,
            },
        },
        CastType::Float | CastType::Double | CastType::Real => Ty::Double,
        CastType::Money => Ty::Money,
        CastType::Date => Ty::Date,
        CastType::Time => Ty::Time,
        CastType::TimeStamp => Ty::TimeStamp,
        CastType::Guid => Ty::Guid,
    }
}

/// `node` as the logical value an `AND`, `OR` or `NOT` takes of it: the
/// original engine reads any other type as true or false, by what
/// [`crate::eval`] does for the label `TRUTH` (`fixtures/sql/gram_cast2.json`).
fn truth(node: Node) -> Node {
    if node.ty == Ty::Bool || node.ty == Ty::Unknown {
        return node;
    }
    let pos = node.pos;
    Node::new(Kind::Cast(Box::new(node), "TRUTH"), Ty::Bool, pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As the original engine reads them (its texts of the values, to 17
    /// digits, probed with `tools/ace/sql/sql.c`).
    #[test]
    #[allow(clippy::excessive_precision)]
    fn exponent_numbers() {
        for (text, value) in [
            ("1e22", 1e22),
            ("1e25", 9.9999999999999988e24),
            ("3e25", 3.0000000000000001e25),
            ("1e30", 9.9999999999999988e29),
            ("1E30", 9.9999999999999988e29),
            ("10e29", 9.9999999999999988e29),
            ("0.1e31", 9.9999999999999988e29),
            ("1.5e30", 1.4999999999999999e30),
            ("2.5e30", 2.4999999999999996e30),
            ("1e40", 9.9999999999999979e39),
            ("1.2345678901234567e30", 1.2345678901234566e30),
            ("123.456e0", 123.45599999999999),
            ("0.123456789e5", 12345.678900000003),
            ("5.551115123125783e0", 5.5511151231257845),
            ("81644.58073e-3", 81.644580729999987),
            ("12345.678e10", 123456780000000.0),
            ("1.2345e3", 1234.5),
        ] {
            assert_eq!(exponent_number(text), Some(value), "{text}");
        }
        assert_eq!(exponent_number("1e400"), Some(f64::INFINITY));
        assert_eq!(exponent_number("1e-400"), Some(0.0));
        assert_eq!(exponent_number(".e5"), None);
    }
}
