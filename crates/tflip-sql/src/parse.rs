//! Tokens to a [`Statement`], by recursive descent.
//!
//! Operator precedence, loosest first: `OR`, `AND`, `NOT`, the predicates
//! (comparisons, `IS NULL`, `BETWEEN`, `IN`, `LIKE`), the bitwise `|`, `&`
//! and `^`, `+` and `-`, `*`, `/` and `%`, and the unary `-`, `+` and `~`.

use crate::ast::*;
use crate::compile::typed_text;
use crate::error::{Pos, SqlError};
use crate::lex::{Tok, Token, lex};

/// A parsed statement and its parameters: the name of each (`None` for
/// `?`), in order of appearance.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub statement: Statement,
    pub params: Vec<Option<String>>,
}

/// The read-only variables of the error a `CATCH` handles.
pub const ERROR_VARIABLES: [&str; 3] = ["__errcode", "__errtext", "__errclass"];

/// A parser over the tokens of `text`.
fn parser(text: &str) -> Result<Parser, SqlError> {
    let tokens = lex(text)?;
    // An error at the end of the text is at its last character.
    let end = text.chars().count().saturating_sub(1);
    let source = text.chars().collect();
    Ok(Parser {
        tokens,
        i: 0,
        end,
        params: Vec::new(),
        context: Context::None,
        tables: 0,
        source,
        sent: false,
        body_started: false,
        // The variables of the error a `CATCH` handles are always there.
        declared: ERROR_VARIABLES.iter().map(|v| (*v).to_owned()).collect(),
        anchor: None,
        in_call: 0,
        sum_pos: 0,
        unreachable: None,
        cursors: Vec::new(),
        try_depth: 0,
        catch_depth: 0,
        loop_depth: 0,
        stored: false,
        in_finally: false,
        in_outer_join: false,
        outer_trailer: String::new(),
    })
}

/// Parses `text`, one statement and an optional `;`, or a script: a text of
/// several statements, or one that starts with a statement of the script
/// language.
pub fn parse(text: &str) -> Result<Parsed, SqlError> {
    let mut parser = parser(text)?;
    if parser.at_punct(";") {
        return Err(SqlError::no_statement());
    }
    if parser.starts_script() {
        return sent_script(text);
    }
    let statement = match parser.show_plan()? {
        true => {
            let Some(start) = parser.peek().map(|t| t.pos) else {
                return Err(SqlError::no_statement());
            };
            let inner = parser.statement()?;
            // The statement's text, without the `;` after it.
            let rest: String = text.chars().skip(start).collect();
            let rest = rest.trim_end();
            let rest = rest.strip_suffix(';').unwrap_or(rest).trim_end();
            Statement::ShowPlan(Box::new(inner), rest.to_owned())
        }
        false => parser.statement()?,
    };
    let semicolon = parser.eat_punct(";");
    if let Some(token) = parser.peek() {
        // Statements after the first one: the text is a script.
        if semicolon {
            return sent_script(text);
        }
        return Err(SqlError::unexpected_token(
            &token.tok.unexpected_text(),
            parser.pos(),
        ));
    }
    Ok(Parsed {
        statement,
        params: parser.params,
    })
}

/// Parses `text` as a script sent as a statement: its statements end with
/// `;`, but the last one's may be left out.
fn sent_script(text: &str) -> Result<Parsed, SqlError> {
    let mut parser = parser(text)?;
    parser.sent = true;
    let script = parser.script()?;
    if let Some(token) = parser.peek().cloned() {
        // An end nothing opened is a statement that is none.
        if BLOCK_ENDS.iter().any(|w| token.tok.is(w)) {
            parser.statement()?;
        }
        return Err(SqlError::unexpected_token(
            &token.tok.unexpected_text(),
            token.pos,
        ));
    }
    Ok(Parsed {
        statement: Statement::Script(script),
        params: parser.params,
    })
}

/// A view's `SELECT`, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewSelect {
    /// The text the dictionary keeps: from the first word to the last token,
    /// without the comments and the `;` around it (`fixtures/sql/ddviews`).
    pub text: String,
    pub select: Select,
    /// How many parameters it has, which a view cannot (2141).
    pub params: usize,
}

/// What `AdsDDAddView` makes of the view `name`'s `SELECT` `text` before it
/// looks at the tables: that it is a `SELECT` that parses, and sorts only
/// with a `TOP` (`fixtures/sql/ddviews.json`, `views.json`).
pub fn check_view(name: &str, text: &str) -> Result<ViewSelect, SqlError> {
    let tokens = lex(text)?;
    // The engine counts these positions in a statement it makes of the
    // view's, whose prefix is 18 characters and the name.
    let prefix = 18 + name.chars().count();
    if !tokens.first().is_some_and(|t| t.tok.is("SELECT")) {
        return Err(SqlError::expected_explained(
            "SELECT",
            NEEDS_SELECT,
            tokens.first().map_or(0, |t| t.pos) + prefix,
        ));
    }
    let Parsed {
        statement: Statement::Select(select),
        params,
    } = parse(text)?
    else {
        return Err(SqlError::expected_explained("SELECT", NEEDS_SELECT, prefix));
    };
    match view_order(&tokens, &select) {
        Some(ViewOrder::Top(pos)) => return Err(SqlError::unexpected(NEEDS_TOP, pos + prefix)),
        Some(ViewOrder::Nested(pos, why)) => {
            return Err(SqlError::expected_explained(")", &why, pos));
        }
        None => {}
    }
    let source: Vec<char> = text.chars().collect();
    Ok(ViewSelect {
        text: view_text(&tokens, &source),
        select: *select,
        params: params.len(),
    })
}

/// What a view's `SELECT` set through a property is refused for.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewRefusal {
    /// A statement that does not parse, or a derived table or subquery that
    /// sorts.
    Sql(SqlError),
    /// A statement that is not a `SELECT`.
    NotSelect,
    /// A `SELECT` that sorts.
    Order,
}

/// What the system procedure that sets a view's statement makes of `text`
/// before it looks at the tables: that it is a `SELECT` that parses and does
/// not sort (`fixtures/sql/spddvprops.json`). It may have parameters.
pub fn check_view_property(text: &str) -> Result<ViewSelect, ViewRefusal> {
    let tokens = lex(text).map_err(ViewRefusal::Sql)?;
    let Parsed {
        statement: Statement::Select(select),
        params,
    } = parse(text).map_err(ViewRefusal::Sql)?
    else {
        return Err(ViewRefusal::NotSelect);
    };
    if let Some(ViewOrder::Nested(pos, why)) = view_order(&tokens, &select) {
        return Err(ViewRefusal::Sql(SqlError::expected_explained(
            ")", &why, pos,
        )));
    }
    if !select.order.is_empty() {
        return Err(ViewRefusal::Order);
    }
    let source: Vec<char> = text.chars().collect();
    Ok(ViewSelect {
        text: view_text(&tokens, &source),
        select: *select,
        params: params.len(),
    })
}

const NEEDS_SELECT: &str = " You need the SELECT keyword after the AS keyword for CREATE VIEW.";
const NEEDS_TOP: &str = "ORDER : ORDER BY clause not legal in VIEW without TOP x";

/// The text of a view's `SELECT` from `tokens`, which are all of its own
/// but a `;` at the end.
fn view_text(tokens: &[Token], source: &[char]) -> String {
    let tokens = match tokens.last() {
        Some(t) if matches!(t.tok, Tok::Punct(";")) => &tokens[..tokens.len() - 1],
        _ => tokens,
    };
    match (tokens.first(), tokens.last()) {
        (Some(first), Some(last)) => source[first.pos..last.end].iter().collect(),
        _ => String::new(),
    }
}

/// Where a view's `SELECT` sorts without a `TOP`.
enum ViewOrder {
    /// The statement's own `ORDER BY`, at the position.
    Top(Pos),
    /// That of a derived table or subquery, which the engine takes for a
    /// missing `)`: the position and the words that explain each level.
    Nested(Pos, String),
}

/// The first `ORDER BY` of a view's `SELECT` (`tokens`, parsed as `select`)
/// that has no `TOP` in its own `SELECT`.
fn view_order(tokens: &[Token], select: &Select) -> Option<ViewOrder> {
    // The `SELECT`s the parentheses open (None for any other parenthesis),
    // each with whether it has a `TOP` and what its parenthesis follows.
    let mut open: Vec<Option<(bool, &str)>> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        match &token.tok {
            Tok::Punct("(") => {
                let at = |n: usize| tokens.get(i + n).map(|t| &t.tok);
                open.push(match at(1) {
                    Some(t) if t.is("SELECT") => {
                        let mut n = 2;
                        while at(n).is_some_and(|t| t.is("DISTINCT") || t.is("ALL")) {
                            n += 1;
                        }
                        let top = at(n).is_some_and(|t| t.is("TOP"));
                        let after = match i.checked_sub(1).map(|j| &tokens[j].tok) {
                            Some(t) if t.is("FROM") || t.is("JOIN") || *t == Tok::Punct(",") => {
                                " There was a problem parsing the table names after the FROM \
                                 keyword."
                            }
                            _ => " There was a problem parsing the WHERE clause.",
                        };
                        Some((top, after))
                    }
                    _ => None,
                });
            }
            Tok::Punct(")") => {
                open.pop();
            }
            tok if tok.is("ORDER") => match open.iter().rev().flatten().next() {
                None if select.top.is_none() => return Some(ViewOrder::Top(token.pos)),
                Some((false, _)) => {
                    let why: String = open.iter().rev().flatten().map(|(_, w)| *w).collect();
                    return Some(ViewOrder::Nested(token.pos, why));
                }
                _ => {}
            },
            _ => {}
        }
    }
    None
}

/// What the original engine adds to a syntax error of the `SELECT` of `CREATE
/// VIEW`: its sentence goes before the location, unless the message goes on
/// after it.
fn in_view_select(mut error: SqlError) -> SqlError {
    const MORE: &str = " There was an error processing the view SELECT statement";
    const AT: &str = " -- Location of error in the SQL statement is: ";
    if error.native == 2115 {
        match error.message.rfind(AT) {
            Some(i)
                if error.message[i + AT.len()..]
                    .bytes()
                    .all(|b| b.is_ascii_digit()) =>
            {
                error.message.insert_str(i, MORE);
            }
            _ => error.message.push_str(MORE),
        }
    }
    error
}

/// Parses a SQL script — a stored procedure or trigger body — into a sequence of
/// [`ScriptStmt`]s: the control-flow and variable statements of the script
/// language, and the ordinary SQL statements it runs.
/// Whether `statement` ends in a `WHERE` or `SET` expression, which the
/// original reads up to the next `;`, so that its text, as the query log keeps
/// it, has the blanks before it (probed, `fixtures/sql/spddql4.json`).
fn ends_in_condition(statement: &Statement) -> bool {
    let select = |select: &Select| {
        select.filter.is_some()
            && select.unions.is_empty()
            && select.group.is_empty()
            && select.having.is_none()
            && select.order.is_empty()
    };
    match statement {
        Statement::Update(update) => update.filter.is_some() || update.from.is_none(),
        Statement::Delete(delete) => delete.filter.is_some(),
        Statement::Select(s) => select(s),
        Statement::Insert(insert) => match &insert.source {
            InsertSource::Select(s) => select(s),
            InsertSource::Values(_) => false,
        },
        _ => false,
    }
}

pub fn parse_script(text: &str) -> Result<Vec<ScriptStmt>, SqlError> {
    let mut parser = parser(text)?;
    let script = parser.script()?;
    if let Some(token) = parser.peek() {
        return Err(SqlError::unexpected_token(
            &token.tok.unexpected_text(),
            token.pos,
        ));
    }
    Ok(script)
}

const INSERT_VALUES_PROBLEM: &str =
    " There was a problem parsing the value list in the INSERT INTO statement.";
const SET_LIST_PROBLEM: &str = " There were problems parsing the SET list in the UPDATE statement.";

/// What the original engine says of a problem in the tables after the `FROM`
/// of an `UPDATE`.
const UPDATE_FROM_PROBLEM: &str =
    " There was a problem parsing the table names after the FROM keyword in your UPDATE statement.";

/// The table an `UPDATE` with a `FROM` changes: the table or alias of the
/// `FROM` that its name names, or else the table of that name that the
/// `FROM` has once (with an alias), or `table` itself, joined to the `FROM`'s.
fn update_target(table: TableRef, from: &From) -> Result<TableRef, SqlError> {
    fn tables<'a>(from: &'a From, out: &mut Vec<&'a TableRef>) {
        match from {
            From::Table(t) => out.push(t),
            From::Join { left, right, .. } => {
                tables(left, out);
                tables(right, out);
            }
            From::Derived { .. } | From::Procedure { .. } => {}
        }
    }
    let mut all = Vec::new();
    tables(from, &mut all);
    let name = |t: &TableRef| crate::query::short_name(&t.name).to_owned();
    let wanted = table.name.as_str();
    let labelled = all.iter().find(|t| {
        t.alias
            .as_deref()
            .unwrap_or(&name(t))
            .eq_ignore_ascii_case(wanted)
    });
    if let Some(found) = labelled {
        return Ok((*found).clone());
    }
    let mut named = all.iter().filter(|t| name(t).eq_ignore_ascii_case(wanted));
    match (named.next(), named.next()) {
        (Some(found), None) => Ok((*found).clone()),
        (Some(_), Some(_)) => Err(SqlError::update_table_twice(wanted)),
        _ => Ok(table),
    }
}

/// What is being parsed, for the end of syntax error messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    None,
    /// The select list's column `n` (1-based).
    Column(usize),
    Where,
    /// The tables after `FROM`.
    From,
    /// The tables after `FROM` of an `UPDATE`.
    UpdateFrom,
    /// The `WHERE` of an `UPDATE`.
    UpdateWhere,
    /// The `WHERE` of a `DELETE`.
    DeleteWhere,
    GroupBy,
    OrderBy,
    Having,
    /// The value list of an `INSERT`.
    InsertValues,
    /// The `SET` list of an `UPDATE`.
    SetList,
}

/// The scalar functions of the original engine, which a user-defined function
/// may not be named after (`CREATE FUNCTION` refuses them with 2230).
const SCALAR_FUNCTIONS: [&str; 115] = [
    "ABS",
    "ACOS",
    "ALLTRIM",
    "APPLICATIONID",
    "ASCII",
    "ASIN",
    "ATAN",
    "ATAN2",
    "BASE64DECODE",
    "BASE64ENCODE",
    "BIT_LENGTH",
    "CAST",
    "CEILING",
    "CHAR",
    "CHAR2HEX",
    "CHARACTER_LENGTH",
    "CHAR_LENGTH",
    "COALESCE",
    "CONCAT",
    "CONTAINS",
    "CONVERT",
    "COS",
    "COT",
    "CREATETIMESTAMP",
    "CTOD",
    "CTOT",
    "CTOTS",
    "CURDATE",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_TIMESTAMP_UTC",
    "CURTIME",
    "DATABASE",
    "DATE",
    "DAY",
    "DAYNAME",
    "DAYOFMONTH",
    "DAYOFWEEK",
    "DAYOFYEAR",
    "DEGREES",
    "DIFFERENCE",
    "DTOC",
    "DTOS",
    "EMPTY",
    "ENDSWITH",
    "EXP",
    "EXTRACT",
    "FLOOR",
    "FRAC_SECOND",
    "HEX2CHAR",
    "HOUR",
    "IFNULL",
    "IIF",
    "ISNULL",
    "ISOWEEK",
    "LASTAUTOINC",
    "LASTROWID",
    "LASTROWVERSION",
    "LCASE",
    "LEN",
    "LENGTH",
    "LOCATE",
    "LOG",
    "LOG10",
    "LOWER",
    "LTRIM",
    "MINUTE",
    "MOD",
    "MONTH",
    "MONTHNAME",
    "NEWID",
    "NEWIDSTRING",
    "NOW",
    "OCTET_LENGTH",
    "PI",
    "POSITION",
    "POWER",
    "QUARTER",
    "RADIANS",
    "RAND",
    "RAT",
    "REPEAT",
    "REPLACE",
    "REVERSE",
    "ROUND",
    "ROWNUM",
    "RTRIM",
    "SCORE",
    "SECOND",
    "SIGN",
    "SIN",
    "SOUNDEX",
    "SPACE",
    "SQRT",
    "STARTSWITH",
    "STOD",
    "STOTS",
    "STR",
    "STRZERO",
    "SUBSTRING",
    "SUBSTRINGOF",
    "TAN",
    "TIME",
    "TIMESTAMPADD",
    "TIMESTAMPDIFF",
    "TRIM",
    "TRUNCATE",
    "TSTOD",
    "TTOC",
    "UCASE",
    "UPPER",
    "VAL",
    "WEEK",
    "YEAR",
];

/// Whether `name` is a scalar function's.
fn is_scalar_function(name: &str) -> bool {
    SCALAR_FUNCTIONS
        .iter()
        .any(|f| f.eq_ignore_ascii_case(name))
}

/// A result type as a function's definition keeps it: the name as written
/// and the size of a type that has one, `CHAR ( 20 )`, `NUMERIC ( 10, 3 )`,
/// `DOUBLE ( 15 )`.
fn spelled_type(name: &str, sizes: &[u32]) -> String {
    let upper = name.to_ascii_uppercase();
    match upper.as_str() {
        "CHAR" | "CICHAR" | "VARCHAR" | "RAW" | "VARBINARY" | "NCHAR" | "NVARCHAR" => {
            format!("{name} ( {} )", sizes.first().unwrap_or(&0))
        }
        "NUMERIC" => format!(
            "{name} ( {}, {} )",
            sizes.first().unwrap_or(&0),
            sizes.get(1).unwrap_or(&0)
        ),
        // A double shows its decimals: 15 and a currency double's 2 when it
        // is given none.
        "DOUBLE" | "CURDOUBLE" => {
            let default = if upper == "DOUBLE" { 15 } else { 2 };
            format!("{name} ( {} )", sizes.last().unwrap_or(&default))
        }
        _ => name.to_owned(),
    }
}

/// Words that end an expression or select item rather than name a column
/// alias. `CROSS` is none: the original engine has no `CROSS JOIN` and
/// reads it as an alias (`fixtures/sql/gram_cross.json`).
const RESERVED: [&str; 27] = [
    "FROM", "WHERE", "ORDER", "GROUP", "HAVING", "UNION", "INTO", "AS", "AND", "OR", "NOT", "IS",
    "IN", "LIKE", "BETWEEN", "ON", "JOIN", "INNER", "LEFT", "RIGHT", "FULL", "OUTER", "ESCAPE",
    "AT", "USING", "SELECT", "END",
];

/// The reserved SQL words, in the original engine's order (not quite
/// alphabetical — `FUNCTION` before `FULL`, `REVOKE` before `RETURN`), as
/// `sp_GetSQLKeywords` lists them. A bare name may not be one.
pub const KEYWORDS: [&str; 91] = [
    "ADD",
    "ALL",
    "ALTER",
    "AND",
    "ANY",
    "AS",
    "ASC",
    "AT",
    "AVG",
    "BEGIN",
    "BETWEEN",
    "BY",
    "CASE",
    "CLOSE",
    "COLUMN",
    "COMMIT",
    "CONSTRAINT",
    "COUNT",
    "CREATE",
    "CURSOR",
    "DECLARE",
    "DEFAULT",
    "DELETE",
    "DESC",
    "DISTINCT",
    "DROP",
    "ELSE",
    "END",
    "ESCAPE",
    "EXECUTE",
    "EXISTS",
    "FALSE",
    "FETCH",
    "FOR",
    "FROM",
    "FUNCTION",
    "FULL",
    "GRANT",
    "GROUP",
    "HAVING",
    "IN",
    "INDEX",
    "INNER",
    "INSERT",
    "INTO",
    "IS",
    "JOIN",
    "KEY",
    "LEFT",
    "LIKE",
    "MAX",
    "MERGE",
    "MIN",
    "NOT",
    "NULL",
    "OF",
    "ON",
    "OPEN",
    "OR",
    "ORDER",
    "OUTER",
    "OUTPUT",
    "PRIMARY",
    "PROCEDURE",
    "REVOKE",
    "RETURN",
    "RETURNS",
    "RIGHT",
    "ROLLBACK",
    "SAVEPOINT",
    "SELECT",
    "SET",
    "SQL",
    "SUM",
    "TABLE",
    "THEN",
    "TO",
    "TRANSACTION",
    "TRIGGER",
    "TRUE",
    "UNION",
    "UNIQUE",
    "UPDATE",
    "USER",
    "USING",
    "VALUES",
    "VARCHAR",
    "VIEW",
    "WHEN",
    "WHERE",
    "WORK",
];

/// The aggregate functions.
const AGGREGATES: [&str; 6] = ["COUNT", "SUM", "AVG", "MIN", "MAX", "GROUP_CONCAT"];

/// Statements that start with these words are SQL tflip does not run yet.
const OTHER_STATEMENTS: [&str; 8] = [
    "CREATE",
    "DROP",
    "ALTER",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "SAVEPOINT",
    "SET",
];

struct Parser {
    tokens: Vec<Token>,
    i: usize,
    /// The position just past the text, where errors at its end are.
    end: Pos,
    params: Vec<Option<String>>,
    context: Context,
    /// The tables named so far.
    tables: usize,
    /// The text, for what a statement keeps as written (a trigger's body).
    source: Vec<char>,
    /// Whether the text is a script sent as a statement, whose last
    /// statement may end without its `;`.
    sent: bool,
    /// Whether a statement other than a `DECLARE` has been parsed.
    body_started: bool,
    /// The variables declared so far, in lower case.
    declared: Vec<String>,
    /// Where the statement just parsed ends the script's flow (`RETURN` or
    /// `RAISE`), and the reason a statement after it fails.
    unreachable: Option<(Pos, &'static str)>,
    /// The cursors declared so far, in lower case.
    cursors: Vec<String>,
    /// How many `TRY` blocks the statement being parsed is in, and `CATCH`
    /// clauses (where `RAISE;` is allowed).
    try_depth: usize,
    catch_depth: usize,
    /// How many `WHILE` loops the statement being parsed is in.
    loop_depth: usize,
    /// Whether a body being stored is parsed: `LEAVE` and `CONTINUE` outside
    /// a loop and statements after one that ends the flow are refused only
    /// when it runs.
    stored: bool,
    /// Whether the block being parsed is a `FINALLY`, which only `END` ends.
    in_finally: bool,
    /// Whether the joins of an `{OJ …}` are being parsed, which hold no other.
    in_outer_join: bool,
    /// What the original engine says of the clause a subselect is in, after
    /// its own (`fixtures/sql/gram_cross.json`).
    outer_trailer: String,
    /// The token before the operand being parsed, where the original engine
    /// places the error of a call's missing `)`, and how many calls' arguments
    /// are being parsed (their operands move it no more).
    anchor: Option<Pos>,
    in_call: usize,
    /// Where the last `+` or `-` of a sum was.
    sum_pos: Pos,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.i)
    }

    fn peek_at(&self, ahead: usize) -> Option<&Tok> {
        self.tokens.get(self.i + ahead).map(|t| &t.tok)
    }

    /// The position of the next token, or of the end. The original engine
    /// places an error at a `.` just after it (probed,
    /// `fixtures/sql/links.json`).
    fn pos(&self) -> Pos {
        match self.peek() {
            Some(Token {
                tok: Tok::Punct(p),
                pos,
                ..
            }) if *p == "." => pos + 1,
            token => token.map_or(self.end, |t| t.pos),
        }
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.i).cloned();
        self.i += 1;
        token
    }

    fn at_word(&self, word: &str) -> bool {
        self.peek().is_some_and(|t| t.tok.is(word))
    }

    fn eat_word(&mut self, word: &str) -> bool {
        let found = self.at_word(word);
        if found {
            self.i += 1;
        }
        found
    }

    fn at_punct(&self, p: &str) -> bool {
        matches!(self.peek(), Some(Token { tok: Tok::Punct(q), .. }) if *q == p)
    }

    fn eat_punct(&mut self, p: &str) -> bool {
        let found = self.at_punct(p);
        if found {
            self.i += 1;
        }
        found
    }

    /// The end of a syntax error message for the current context.
    fn trailer(&self) -> String {
        Self::trailer_in(self.context) + &self.outer_trailer
    }

    fn trailer_in(context: Context) -> String {
        match context {
            Context::None => String::new(),
            Context::Column(n) => {
                format!(" [Parsing Expression (column {n} in the SELECT clause)]")
            }
            Context::Where => " There was a problem parsing the WHERE clause.".to_owned(),
            Context::From => {
                " There was a problem parsing the table names after the FROM keyword.".to_owned()
            }
            Context::UpdateFrom => UPDATE_FROM_PROBLEM.to_owned(),
            Context::UpdateWhere => {
                " There were problems parsing the where clause in your UPDATE statement.".to_owned()
            }
            Context::DeleteWhere => {
                " There was an error parsing the WHERE clause in DELETE FROM. ".to_owned()
            }
            Context::GroupBy => " There was a problem parsing the GROUP BY clause.".to_owned(),
            Context::OrderBy => {
                " There was a problem parsing the ORDER BY clause in your SELECT statement."
                    .to_owned()
            }
            Context::Having => " There was a problem parsing the HAVING clause.".to_owned(),
            Context::InsertValues => INSERT_VALUES_PROBLEM.to_owned(),
            Context::SetList => SET_LIST_PROBLEM.to_owned(),
        }
    }

    /// What the original engine says of a problem in the table names after
    /// `FROM`.
    fn table_names_problem(&self) -> String {
        match self.context {
            Context::UpdateFrom => UPDATE_FROM_PROBLEM,
            _ => " There was a problem parsing the table names after the FROM keyword.",
        }
        .to_owned()
            + &self.outer_trailer
    }

    /// 2115: `what` expected at the next token.
    fn expected(&self, what: &str) -> SqlError {
        SqlError::expected(what, self.pos(), &self.trailer())
    }

    /// Where a missing keyword or punctuation is: at the next token, or at
    /// the start of the last one at the end of the text (an expression or
    /// name missing there is at the last character instead).
    fn pos_expected(&self) -> Pos {
        match self.peek() {
            None => {
                // Blanks after the last token are a lexeme of their own to it.
                let kept = self.source.iter().rposition(|c| !c.is_whitespace());
                match kept.map_or(0, |i| i + 1) {
                    blanks if blanks < self.source.len() => blanks,
                    _ => self.tokens.last().map_or(self.end, |t| t.pos),
                }
            }
            Some(_) => self.pos(),
        }
    }

    fn expect_punct(&mut self, p: &str) -> Result<(), SqlError> {
        if self.eat_punct(p) {
            return Ok(());
        }
        // The original engine words a missing `)` with the context first.
        let pos = self.pos_expected();
        Err(SqlError::expected_explained(p, &self.trailer(), pos))
    }

    fn expect_word(&mut self, word: &str) -> Result<(), SqlError> {
        if self.eat_word(word) {
            Ok(())
        } else {
            Err(SqlError::expected(
                word,
                self.pos_expected(),
                &self.trailer(),
            ))
        }
    }

    /// [`Parser::expect_word`] for the words of a condition (`IS NULL`,
    /// `BETWEEN … AND`), which the original engine words with the context
    /// first.
    fn expect_word_explained(&mut self, word: &str) -> Result<(), SqlError> {
        if self.eat_word(word) {
            return Ok(());
        }
        let pos = self.pos_expected();
        Err(SqlError::expected_explained(word, &self.trailer(), pos))
    }

    /// `SHOW PLAN FOR`, before a statement whose plan is wanted.
    fn show_plan(&mut self) -> Result<bool, SqlError> {
        if !self.at_word("SHOW") {
            return Ok(false);
        }
        self.i += 1;
        // Only the two words together, in this order, are a plan too.
        if self.at_word("ALL") && self.at_word_at(1, "INTERNAL") && self.at_word_at(2, "PLAN") {
            self.i += 2;
        }
        self.expect_word("PLAN")?;
        self.expect_word("FOR")?;
        Ok(true)
    }

    /// Whether the text starts with a statement of the script language,
    /// which makes it a script even when it is only one statement.
    fn starts_script(&self) -> bool {
        const WORDS: [&str; 13] = [
            "DECLARE", "IF", "WHILE", "RETURN", "OPEN", "FETCH", "CLOSE", "TRY", "CACHE", "FOR",
            "DBREAK", "LEAVE", "CONTINUE",
        ];
        WORDS.iter().any(|w| self.at_word(w))
            || (self.at_word("RAISE")
                && matches!(
                    self.peek_at(1),
                    None | Some(Tok::Ident(_) | Tok::Quoted(_) | Tok::Punct(";"))
                ))
            || (self.at_word("EXECUTE")
                && (self.at_word_at(1, "IMMEDIATE")
                    || self.at_word_at(1, "PRINT")
                    || self.at_word_at(1, "DPRINT")))
            || self.at_assignment()
    }

    /// Whether `name = ` starts here.
    fn at_assignment(&self) -> bool {
        matches!(
            self.peek().map(|t| &t.tok),
            Some(Tok::Ident(_) | Tok::Quoted(_))
        ) && matches!(self.peek_at(1), Some(Tok::Punct(p)) if *p == "=")
    }

    /// A sequence of script statements, up to the end of the input or a block
    /// terminator (`END`, `ELSE`, `ELSEIF` or `ENDIF`, `ENDWHILE`, `ENDTRY`),
    /// which the caller consumes.
    fn script(&mut self) -> Result<Vec<ScriptStmt>, SqlError> {
        let mut out = Vec::new();
        while self.peek().is_some()
            && !BLOCK_ENDS.iter().any(|w| self.at_word(w))
            && !(self.try_depth > 0
                && !self.in_finally
                && (self.at_word("CATCH") || self.at_word("FINALLY")))
        {
            let unreachable = self.unreachable.take();
            // What the next statement says of itself comes first.
            let stmt = self.script_stmt()?;
            if let Some((pos, why)) = unreachable.filter(|_| !self.stored) {
                return Err(SqlError::script_error(why, pos));
            }
            out.push(stmt);
        }
        self.unreachable = None;
        Ok(out)
    }

    /// The `;` that ends a script statement; a script sent as a statement
    /// may leave out its last.
    fn end_statement(&mut self) -> Result<(), SqlError> {
        if self.sent {
            return match self.peek() {
                None => Ok(()),
                Some(token) if token.tok != Tok::Punct(";") => Err(SqlError::unexpected_token(
                    &token.tok.unexpected_text(),
                    token.pos,
                )),
                Some(_) => self.expect_punct(";"),
            };
        }
        self.expect_punct(";")
    }

    /// What follows the condition of an `IF`: `THEN`, its statements, any
    /// `ELSEIF` with its own, an optional `ELSE` and its statements, and
    /// `END IF;`.
    fn if_tail(&mut self) -> Result<(Vec<ScriptStmt>, Vec<ScriptStmt>), SqlError> {
        let branches = self.if_branches()?;
        self.end_block("IF")?;
        Ok(branches)
    }

    /// `THEN`, the statements, and what an `ELSEIF` or `ELSE` after them
    /// has, an `ELSEIF` being an `IF` of the `ELSE` that does not end
    /// before the `IF`'s own end.
    fn if_branches(&mut self) -> Result<(Vec<ScriptStmt>, Vec<ScriptStmt>), SqlError> {
        self.expect_word("THEN")?;
        let then = self.nested_script()?;
        if self.eat_word("ELSEIF") || self.eat_word("ELIF") {
            let cond = self.expr()?;
            let (then_if, els_if) = self.if_branches()?;
            let nested = ScriptStmt::If {
                cond,
                then: then_if,
                els: els_if,
            };
            return Ok((then, vec![nested]));
        }
        let els = match self.eat_word("ELSE") {
            true => self.nested_script()?,
            false => Vec::new(),
        };
        Ok((then, els))
    }

    /// What follows the condition of a `WHILE`: `DO`, its statements and
    /// `END WHILE;`.
    fn while_tail(&mut self) -> Result<Vec<ScriptStmt>, SqlError> {
        self.expect_word("DO")?;
        self.loop_depth += 1;
        let body = self.nested_script();
        self.loop_depth -= 1;
        let body = body?;
        self.end_block("WHILE")?;
        Ok(body)
    }

    /// The end of a block of `tag` (`IF`, `WHILE` or `TRY`) and the `;` after
    /// it: `END tag`, `ENDtag` or a bare `END`
    /// (`fixtures/sql/gram_script.json`).
    fn end_block(&mut self, tag: &str) -> Result<(), SqlError> {
        if self.eat_word(&format!("END{tag}")) {
            return self.end_of_block();
        }
        if self.peek().is_none() {
            return Err(SqlError::script_end_expecting(&format!("END [{tag}];")));
        }
        // Another block's end, or an `ELSE` where none can be, is a statement
        // that is none.
        if !self.at_word("END") {
            self.statement()?;
        }
        self.expect_word("END")?;
        if !self.eat_word(tag)
            && let Some(token) = self.peek().filter(|t| t.tok != Tok::Punct(";"))
        {
            // The original puts it at the token after.
            let at = self.tokens.get(self.i + 1).map_or(self.end, |t| t.pos);
            return Err(SqlError::unexpected(
                &format!(
                    "{} : found unexpected tag after END block.  Expecting [ {tag}]. ",
                    token.tok.text()
                ),
                at,
            ));
        }
        self.end_of_block()
    }

    /// The `;` after a block's end. Without it the end of a script that was
    /// not sent as a statement is where the original expects the `END`
    /// of the script, at the last word.
    fn end_of_block(&mut self) -> Result<(), SqlError> {
        if !self.sent && self.peek().is_none() {
            return Err(SqlError::script_end(self.tokens[self.i - 1].pos));
        }
        self.end_statement()
    }

    /// The statements of the body of an `IF`, `WHILE` or block, which a
    /// `CATCH` or `FINALLY` of a `TRY` around it does not end.
    fn nested_script(&mut self) -> Result<Vec<ScriptStmt>, SqlError> {
        let outer = std::mem::replace(&mut self.try_depth, 0);
        let body = self.script();
        self.try_depth = outer;
        body
    }

    /// The name of a variable after `DECLARE`: no SQL keyword, and none of
    /// the script's (`fixtures/sql/claims.json`).
    fn variable_name(&mut self) -> Result<String, SqlError> {
        const SCRIPT_WORDS: [&str; 10] = [
            "WHILE", "TRY", "IF", "CATCH", "FINALLY", "RAISE", "CONTINUE", "LEAVE", "DEBUG",
            "CACHE",
        ];
        let pos = self.pos();
        if let Some(Tok::Ident(word)) = self.peek().map(|t| &t.tok) {
            if KEYWORDS.iter().any(|k| word.eq_ignore_ascii_case(k)) {
                return Err(self.missing_variable_name());
            }
            if SCRIPT_WORDS.iter().any(|k| word.eq_ignore_ascii_case(k)) {
                return Err(SqlError::declaration(
                    &format!(
                        "Variable name cannot be a script reserved keyword. Invalid variable \
                         name: {word}"
                    ),
                    pos,
                ));
            }
        }
        self.name().ok_or_else(|| self.missing_variable_name())
    }

    fn missing_variable_name(&self) -> SqlError {
        SqlError::expected_explained(
            &format!("<identifier> but found: {}", self.found()),
            " -- Missing variable name.",
            self.pos(),
        )
    }

    /// One script statement, terminated by its `;`.
    fn script_stmt(&mut self) -> Result<ScriptStmt, SqlError> {
        if self.at_punct(";") {
            return Err(SqlError::no_statement());
        }
        if self.eat_word("DECLARE") {
            let mut declares = Vec::new();
            loop {
                let pos = self.pos();
                let name = self.variable_name()?;
                if self.body_started {
                    return Err(SqlError::declaration(
                        "Variable declaration is not allowed in the script body.",
                        pos,
                    ));
                }
                let lower = name.to_lowercase();
                if self.declared.contains(&lower) {
                    return Err(SqlError::declaration(
                        &format!("Variable name {name} has already been declared."),
                        pos,
                    ));
                }
                if self.eat_word("CURSOR") {
                    self.declared.push(lower.clone());
                    self.cursors.push(lower);
                    let def = self.cursor_as()?;
                    // Only a `;` or `,` follows, not the end of the text.
                    if !self.at_punct(";") && !self.at_punct(",") {
                        return Err(self.expected(
                            "<symbol> Expecting semicolon or comma in DECLARE statement.",
                        ));
                    }
                    declares.push(ScriptStmt::DeclareCursor { name, def, pos });
                    if !self.eat_punct(",") {
                        break;
                    }
                    continue;
                }
                self.declared.push(lower);
                let ty = self.declare_type(pos)?;
                self.end_of_declaration()?;
                declares.push(ScriptStmt::Declare { name, ty });
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.end_statement()?;
            return Ok(match declares.len() {
                1 => declares.remove(0),
                _ => ScriptStmt::Block(declares),
            });
        }
        self.body_started = true;
        if self.eat_word("IF") {
            if let Some((name, negated, pos)) = self.fetch_condition()? {
                let (then, els) = self.if_tail()?;
                return Ok(ScriptStmt::IfFetch {
                    name,
                    negated,
                    pos,
                    then,
                    els,
                });
            }
            let cond = self.expr()?;
            let (then, els) = self.if_tail()?;
            return Ok(ScriptStmt::If { cond, then, els });
        }
        if self.eat_word("WHILE") {
            if let Some((name, negated, pos)) = self.fetch_condition()? {
                let body = self.while_tail()?;
                return Ok(ScriptStmt::WhileFetch {
                    name,
                    negated,
                    pos,
                    body,
                });
            }
            let cond = self.expr()?;
            let body = self.while_tail()?;
            return Ok(ScriptStmt::While { cond, body });
        }
        if self.eat_word("RETURN") {
            self.unreachable = Some((
                self.pos(),
                "Unreachable statements after loop control statement.",
            ));
            // Only a function returns a value.
            let value = if self.sent || self.at_punct(";") {
                None
            } else {
                Some(self.expr()?)
            };
            self.end_statement()?;
            return Ok(ScriptStmt::Return(value));
        }
        for word in ["LEAVE", "CONTINUE"] {
            if !self.at_word(word) || self.at_assignment() {
                continue;
            }
            if self.loop_depth == 0 && !self.stored {
                if matches!(self.peek_at(1), None | Some(Tok::Punct(";"))) {
                    self.i += 1;
                    let why = format!("Invalid {word} without loop.");
                    return Err(SqlError::script_error(&why, self.pos()));
                }
                continue;
            }
            self.i += 1;
            self.unreachable = Some((
                self.pos(),
                "Unreachable statements after loop control statement.",
            ));
            self.end_statement()?;
            return Ok(match word {
                "LEAVE" => ScriptStmt::Leave,
                _ => ScriptStmt::Continue,
            });
        }
        if self.at_word("RAISE") && matches!(self.peek_at(1), None | Some(Tok::Punct(";"))) {
            self.i += 1;
            let pos = self.pos();
            if self.catch_depth == 0 {
                return Err(SqlError::script_error(
                    "Raising a previously thrown exception is only allowed in the CATCH clause.",
                    pos,
                ));
            }
            self.unreachable = Some((pos, "Unreachable statements after raising exception."));
            self.end_statement()?;
            return Ok(ScriptStmt::Reraise { pos });
        }
        if self.at_word("RAISE") && self.at_word_at(1, "END") {
            self.i += 1;
            return Err(self.expected("Expect exception class identifier but find: END"));
        }
        if self.at_word("RAISE") && matches!(self.peek_at(1), Some(Tok::Ident(_) | Tok::Quoted(_)))
        {
            self.i += 1;
            let pos = self.pos();
            self.unreachable = Some((pos, "Unreachable statements after raising exception."));
            let name = self
                .name()
                .ok_or_else(|| self.expected("an exception name"))?;
            self.expect_punct("(")?;
            let code = self.expr()?;
            self.expect_punct(",")?;
            let message = self.expr()?;
            self.expect_punct(")")?;
            self.end_statement()?;
            return Ok(ScriptStmt::Raise {
                name,
                code,
                message,
                pos,
            });
        }
        // `BEGIN` is a block unless it starts a transaction (`BEGIN TRAN…`);
        // a script sent as a statement has no blocks.
        if !self.sent
            && self.at_word("BEGIN")
            && !self.at_word_at(1, "TRANSACTION")
            && !self.at_word_at(1, "TRAN")
        {
            self.i += 1;
            // A block starts with its own declarations.
            self.body_started = false;
            let body = self.nested_script()?;
            self.body_started = true;
            self.expect_word("END")?;
            self.expect_punct(";")?;
            return Ok(ScriptStmt::Block(body));
        }
        // `SET name = expr` assigns too, but a system variable (`SET
        // ::scope.name = expr`) is the statement.
        let mut set = false;
        if self.at_word("SET")
            && matches!(self.peek_at(1), Some(Tok::Ident(name) | Tok::Quoted(name)) if !name.starts_with("::"))
            && matches!(self.peek_at(2), Some(Tok::Punct(p)) if *p == "=")
        {
            self.i += 1;
            set = true;
        }
        // An assignment is `name = expr`; anything else is a SQL statement.
        if self.at_assignment() {
            let pos = self.pos();
            let name = self.name().unwrap();
            if ERROR_VARIABLES.contains(&name.to_lowercase().as_str()) {
                return Err(SqlError::read_only(&name, pos));
            }
            // A script sent as a statement has no parameters to name a
            // variable it did not declare.
            if self.sent && !self.declared.contains(&name.to_lowercase()) {
                return Err(SqlError::variable_not_found(&name, pos));
            }
            self.i += 1; // `=`
            let value = self.value()?;
            self.end_statement()?;
            return Ok(ScriptStmt::Assign {
                name,
                value,
                pos,
                set,
            });
        }
        // The debugger's break is nothing to run without one.
        if self.eat_word("DBREAK") {
            self.end_statement()?;
            return Ok(ScriptStmt::Block(Vec::new()));
        }
        if self.at_word("FOR") {
            return Err(SqlError::driver_not_capable());
        }
        if self.at_word("EXECUTE") && (self.at_word_at(1, "PRINT") || self.at_word_at(1, "DPRINT"))
        {
            self.i += 2;
            let pos = self.pos();
            let expr = self.expr()?;
            self.end_statement()?;
            return Ok(ScriptStmt::Print { expr, pos });
        }
        if self.at_word("EXECUTE") && self.at_word_at(1, "IMMEDIATE") {
            self.i += 2;
            let pos = self.pos();
            let expr = self.expr()?;
            self.end_statement()?;
            return Ok(ScriptStmt::ExecuteImmediate { expr, pos });
        }
        if let Some(done) = self.cursor_or_try()? {
            return Ok(done);
        }
        let pos = self.pos();
        let statement = self.statement()?;
        // The original's text of a statement that ends in a condition goes on
        // to the `;`, blanks and comments included.
        let last = match ends_in_condition(&statement) {
            true => self.peek().map_or(self.end, |token| token.pos),
            false => self.tokens[self.i.saturating_sub(1)].end,
        };
        let text: String = self.source[pos.min(last)..last].iter().collect();
        self.end_statement()?;
        Ok(ScriptStmt::Sql(Box::new(statement), pos, text))
    }

    /// `AS definition`, if the cursor is given one here.
    fn cursor_as(&mut self) -> Result<Option<Box<Statement>>, SqlError> {
        match self.eat_word("AS") {
            true => Ok(Some(Box::new(self.cursor_definition()?))),
            false => Ok(None),
        }
    }

    /// What a cursor is defined as (after `AS`): a `SELECT` or an `EXECUTE
    /// PROCEDURE`.
    fn cursor_definition(&mut self) -> Result<Statement, SqlError> {
        if self.eat_word("SELECT") {
            return Ok(Statement::Select(Box::new(self.select_into(false)?)));
        }
        if self.eat_word("EXECUTE") {
            return Ok(Statement::Execute(self.execute()?));
        }
        Err(self.expected(&format!(
            "Cursor must be either SELECT or EXECUTE PROCEDURE. Found: {}",
            self.found_or(";")
        )))
    }

    /// The name of a cursor declared before, at the next token.
    fn cursor_name(&mut self) -> Result<(String, Pos), SqlError> {
        let pos = self.pos();
        let name = self.name().ok_or_else(|| self.expected("a cursor name"))?;
        match self.cursors.contains(&name.to_lowercase()) {
            true => Ok((name, pos)),
            false => Err(SqlError::variable_not_found(&name, pos)),
        }
    }

    /// `[NOT] FETCH cursor`, the condition of an `IF` or `WHILE`: the cursor,
    /// whether the condition is negated and where the cursor's name is.
    fn fetch_condition(&mut self) -> Result<Option<(String, bool, Pos)>, SqlError> {
        let negated = self.at_word("NOT") && self.at_word_at(1, "FETCH");
        if !negated && !self.at_word("FETCH") {
            return Ok(None);
        }
        // A cursor that is closed is an error at the `FETCH`.
        self.i += usize::from(negated);
        let pos = self.pos();
        self.i += 1;
        let name_pos = self.pos();
        let name = self.name().ok_or_else(|| self.expected("a cursor name"))?;
        if !self.cursors.contains(&name.to_lowercase()) {
            return Err(SqlError::cursor_not_declared(&name, name_pos));
        }
        Ok(Some((name, negated, pos)))
    }

    /// `OPEN`, `FETCH`, `CLOSE` and `TRY`, if one starts here.
    fn cursor_or_try(&mut self) -> Result<Option<ScriptStmt>, SqlError> {
        if self.eat_word("OPEN") {
            let (name, pos) = self.cursor_name()?;
            let def = self.cursor_as()?;
            self.end_statement()?;
            return Ok(Some(ScriptStmt::OpenCursor { name, def, pos }));
        }
        if self.eat_word("FETCH") {
            let (name, pos) = self.cursor_name()?;
            self.end_statement()?;
            return Ok(Some(ScriptStmt::FetchCursor { name, pos }));
        }
        if self.eat_word("CLOSE") {
            let (name, pos) = self.cursor_name()?;
            self.end_statement()?;
            return Ok(Some(ScriptStmt::CloseCursor { name, pos }));
        }
        if self.eat_word("TRY") {
            return self.try_statement().map(Some);
        }
        Ok(None)
    }

    /// `TRY … CATCH … FINALLY … END [TRY];`, after its `TRY`.
    fn try_statement(&mut self) -> Result<ScriptStmt, SqlError> {
        let outer_try = std::mem::replace(&mut self.try_depth, 1);
        let (outer_finally, outer_catch) = (self.in_finally, self.catch_depth);
        self.in_finally = false;
        let body = self.script()?;
        let mut catches: Vec<Catch> = Vec::new();
        while self.at_word("CATCH") {
            self.i += 1;
            let pos = self.pos();
            if catches.iter().any(|c| c.class.is_none()) {
                return Err(SqlError::script_error(
                    "Catch blocks after CATCH ALL are not reachable.",
                    pos,
                ));
            }
            let class = match self.eat_word("ALL") {
                true => None,
                false => match self.peek().map(|t| t.tok.clone()) {
                    Some(Tok::Ident(name) | Tok::Quoted(name)) if !self.at_word("END") => {
                        self.i += 1;
                        Some(name)
                    }
                    _ => {
                        return Err(self.expected(&format!(
                            "<exception name> but found keyword: {}",
                            self.found_or(";")
                        )));
                    }
                },
            };
            self.catch_depth = outer_catch + 1;
            let body = self.script()?;
            self.catch_depth = outer_catch;
            catches.push(Catch { class, body });
        }
        let finally = match self.eat_word("FINALLY") {
            true => {
                self.in_finally = true;
                let body = self.script()?;
                self.in_finally = false;
                Some(body)
            }
            false => None,
        };
        self.try_depth = outer_try;
        self.in_finally = outer_finally;
        if self.peek().is_none() {
            return Err(SqlError::script_end_expecting("END [TRY];"));
        }
        let pos = self.pos();
        if catches.is_empty()
            && finally.is_none()
            && (self.at_word("END") || self.at_word("ENDTRY"))
        {
            return Err(SqlError::script_error(
                "TRY statement must have at least one CATCH or FINALLY block.",
                pos,
            ));
        }
        self.end_block("TRY")?;
        Ok(ScriptStmt::Try {
            body,
            catches,
            finally,
        })
    }

    /// A declaration ends at its `;` or goes on after a `,`.
    fn end_of_declaration(&self) -> Result<(), SqlError> {
        match self.peek() {
            None
            | Some(Token {
                tok: Tok::Punct(";" | ","),
                ..
            }) => Ok(()),
            Some(Token {
                tok: Tok::Punct(p),
                pos,
                ..
            }) => Err(SqlError::expected_explained(
                "",
                &format!(" expecting <;> or <,> but found <{p}>"),
                *pos,
            )),
            Some(token) => Err(SqlError::expected_explained(
                "<symbol>",
                " Expecting semicolon or comma in DECLARE statement.",
                token.pos,
            )),
        }
    }

    /// The sizes after a type's name, `(n[, m])`, if it has them.
    fn type_sizes(&mut self) -> Result<Vec<u32>, SqlError> {
        let mut sizes: Vec<u32> = Vec::new();
        if self.eat_punct("(") {
            let first = self.pos();
            loop {
                let number = match self.peek().map(|t| &t.tok) {
                    Some(Tok::Number(n)) if n.bytes().all(|b| b.is_ascii_digit()) => {
                        Some(n.parse().unwrap_or(u32::MAX))
                    }
                    _ => None,
                };
                let Some(n) = number else {
                    return Err(SqlError::malformed_number(
                        &self.found(),
                        sizes.is_empty(),
                        first,
                    ));
                };
                self.i += 1;
                sizes.push(n);
                if sizes.len() == 2 || !self.eat_punct(",") {
                    break;
                }
            }
            if !self.eat_punct(")") {
                return Err(SqlError::expected_explained(
                    "",
                    " expecting ')' in variable declaration.",
                    first,
                ));
            }
        }
        Ok(sizes)
    }

    /// The type of the variable declared at `var_pos`, as written (`CHAR(3)`),
    /// checked as the original checks it (`fixtures/sql/declare.json`): the
    /// names it knows, the sizes each takes and the largest it allows.
    fn declare_type(&mut self, var_pos: Pos) -> Result<String, SqlError> {
        let Some(mut ty) = self.name() else {
            return Err(SqlError::expected_explained(
                &format!("<identifier> but found: {}", self.found()),
                " -- Missing the data type after variable name.",
                var_pos,
            ));
        };
        let sizes = self.type_sizes()?;
        crate::script::check_declared_type(&ty, &sizes, var_pos)?;
        if !sizes.is_empty() {
            let sizes: Vec<String> = sizes.iter().map(u32::to_string).collect();
            ty = format!("{ty}({})", sizes.join(","));
        }
        Ok(ty)
    }

    fn statement(&mut self) -> Result<Statement, SqlError> {
        let Some(first) = self.peek().cloned() else {
            return Err(self.expected("identifier or expression"));
        };
        if first.tok.is("SELECT") {
            let start = self.i;
            self.i += 1;
            let select = self.select_into(true)?;
            // A derived table or subquery that sorts without a `TOP` is
            // refused in every statement, not only a view's
            // (`fixtures/sql/derived.json`); one in the select list is
            // worded otherwise there and passes here.
            let tokens = &self.tokens[start..self.i];
            if let Some(ViewOrder::Nested(pos, why)) = view_order(tokens, &select) {
                let mut depth = 0;
                let from = tokens.iter().find(|t| {
                    match t.tok {
                        Tok::Punct("(") => depth += 1,
                        Tok::Punct(")") => depth -= 1,
                        _ => {}
                    }
                    depth == 0 && t.tok.is("FROM")
                });
                if from.is_some_and(|from| from.pos < pos) {
                    return Err(SqlError::expected_explained(")", &why, pos));
                }
            }
            return Ok(Statement::Select(Box::new(select)));
        }
        if first.tok == Tok::Punct("(")
            && self.tokens[self.i..]
                .iter()
                .find(|t| t.tok != Tok::Punct("("))
                .is_some_and(|t| t.tok.is("SELECT"))
        {
            return Ok(Statement::Select(Box::new(self.grouped_select()?)));
        }
        if first.tok.is("EXECUTE") || first.tok.is("EXEC") {
            self.i += 1;
            return Ok(Statement::Execute(self.execute()?));
        }
        if first.tok.is("DEBUG") {
            self.i += 1;
            return self.debug(first.pos);
        }
        if first.tok.is("CACHE") {
            self.i += 1;
            return self.cache_prepare();
        }
        if first.tok.is("INSERT") {
            self.i += 1;
            return Ok(Statement::Insert(Box::new(self.insert()?)));
        }
        if first.tok.is("UPDATE") {
            self.i += 1;
            return Ok(Statement::Update(Box::new(self.update()?)));
        }
        if first.tok.is("DELETE") {
            self.i += 1;
            return Ok(Statement::Delete(Box::new(self.delete()?)));
        }
        if first.tok.is("MERGE") {
            self.i += 1;
            return Ok(Statement::Merge(Box::new(self.merge()?)));
        }
        if first.tok.is("CREATE")
            && (self.at_word_at(1, "TABLE")
                || self.at_word_at(1, "INDEX")
                || self.at_word_at(1, "UNIQUE")
                || self.at_word_at(1, "ONLINE"))
        {
            self.i += 1;
            return self.create();
        }
        if (first.tok.is("CREATE") || first.tok.is("ALTER")) && self.at_word_at(1, "TRIGGER") {
            self.i += 2;
            return Ok(Statement::CreateTrigger(Box::new(
                self.create_trigger(first.tok.is("ALTER"))?,
            )));
        }
        if first.tok.is("CREATE") && self.at_word_at(1, "VIEW") {
            self.i += 2;
            return self.create_view();
        }
        if first.tok.is("CREATE") && self.at_word_at(1, "DATABASE") {
            self.i += 2;
            return self.create_database();
        }
        if first.tok.is("DROP") && self.at_word_at(1, "VIEW") {
            self.i += 2;
            let pos = self.pos();
            let name = self.grant_name().ok_or_else(|| {
                SqlError::expected_explained(
                    "<identifier>",
                    " Missing the view name after the keyword VIEW in DROP VIEW.",
                    pos,
                )
            })?;
            return Ok(Statement::DropView(Named { name, pos }));
        }
        if (first.tok.is("CREATE") || first.tok.is("ALTER")) && self.at_word_at(1, "PROCEDURE") {
            self.i += 2;
            return self.create_procedure(first.tok.is("ALTER"));
        }
        if first.tok.is("DROP") && self.at_word_at(1, "PROCEDURE") {
            self.i += 2;
            let pos = self.pos();
            let name = self.grant_name().ok_or_else(|| {
                SqlError::expected_explained(
                    "<identifier>",
                    " Missing the procedure name after the keyword PROCEDURE in DROP PROCEDURE.",
                    pos,
                )
            })?;
            return Ok(Statement::DropProcedure(Named { name, pos }));
        }
        if (first.tok.is("CREATE") || first.tok.is("ALTER")) && self.at_word_at(1, "FUNCTION") {
            self.i += 2;
            let create = self.create_function(first.tok.is("ALTER"))?;
            return Ok(Statement::FunctionDdl(Box::new(FunctionDdl::Create(
                create,
            ))));
        }
        if first.tok.is("DROP") && self.at_word_at(1, "FUNCTION") {
            self.i += 2;
            let (package, name) = self.function_name(" -- Missing function name.")?;
            return Ok(Statement::FunctionDdl(Box::new(FunctionDdl::Drop {
                package,
                name,
            })));
        }
        if first.tok.is("CREATE") && self.at_word_at(1, "PACKAGE") {
            self.i += 2;
            return self.create_package();
        }
        if first.tok.is("DROP") && self.at_word_at(1, "PACKAGE") {
            self.i += 2;
            let pos = self.pos();
            let name = self.grant_name().ok_or_else(|| {
                SqlError::expected_explained(
                    "<identifier>",
                    " Missing the package name after the keyword PACKAGE in DROP PACKAGE.",
                    pos,
                )
            })?;
            return Ok(Statement::FunctionDdl(Box::new(FunctionDdl::DropPackage(
                Named { name, pos },
            ))));
        }
        if first.tok.is("DROP") && self.at_word_at(1, "TRIGGER") {
            self.i += 2;
            let pos = self.pos();
            let first = self.name().ok_or_else(|| self.expected("<identifier>"))?;
            let first = Named { name: first, pos };
            if !self.eat_punct(".") {
                return Ok(Statement::DropTrigger(None, first));
            }
            let pos = self.pos();
            let name = self.name().ok_or_else(|| self.expected("<identifier>"))?;
            return Ok(Statement::DropTrigger(Some(first), Named { name, pos }));
        }
        if first.tok.is("DROP") && (self.at_word_at(1, "TABLE") || self.at_word_at(1, "INDEX")) {
            self.i += 1;
            return self.drop();
        }
        if first.tok.is("ALTER") && self.at_word_at(1, "TABLE") {
            self.i += 2;
            return Ok(Statement::AlterTable(Box::new(
                self.alter_table(false, false)?,
            )));
        }
        if first.tok.is("ALTER")
            && (self.at_word_at(1, "ONLINE") || self.at_word_at(1, "ONLINE_WITH_TRANSITION"))
        {
            let transition = self.at_word_at(1, "ONLINE_WITH_TRANSITION");
            self.i += 2;
            if !self.at_word("TABLE") {
                return Err(SqlError::expected(
                    " The keyword TABLE, FUNCTION, PROCEDURE or TRIGGER must follow the keyword \
                     ALTER ",
                    self.grant_pos(),
                    "",
                ));
            }
            self.i += 1;
            return Ok(Statement::AlterTable(Box::new(
                self.alter_table(true, transition)?,
            )));
        }
        if first.tok.is("SET") && !self.at_word_at(1, "TRAN") && !self.at_word_at(1, "TRANSACTION")
        {
            self.i += 1;
            return Ok(Statement::SetVariable(Box::new(
                self.set_variable(first.pos)?,
            )));
        }
        if let Some(statement) = self.transaction()? {
            return Ok(Statement::Transaction(statement));
        }
        if first.tok.is("GRANT") || first.tok.is("REVOKE") {
            self.i += 1;
            return Ok(Statement::Grant(Box::new(
                self.grant(first.tok.is("REVOKE"))?,
            )));
        }
        if first.tok.is("BEGIN") {
            self.i += 1;
            return Err(SqlError::begin_without_transaction(self.pos()));
        }
        if ["CREATE", "DROP", "ALTER"].iter().any(|w| first.tok.is(w)) {
            return Err(self.other_ddl(&first));
        }
        if OTHER_STATEMENTS.iter().any(|w| first.tok.is(w)) {
            return Err(SqlError::unsupported(
                &format!("{} statements", first.tok.text()),
                first.pos,
            ));
        }
        // Only a word is pointed at: a number, a string or a sign is not.
        if !matches!(first.tok, Tok::Ident(_)) {
            return Err(SqlError::no_statement());
        }
        // The words that end a block are known as no statement, and the
        // original lists what it has for them (`fixtures/sql/gram_script.json`).
        if first.tok.is("END") || first.tok.is("ELSE") {
            return Err(SqlError::expected(
                "CREATE, DROP, SELECT, INSERT, UPDATE, ALTER, EXECUTE, DELETE, GRANT, REVOKE, \
                 BEGIN, COMMIT, ROLLBACK, SET, MERGE or DEBUG",
                first.pos,
                "",
            ));
        }
        Err(SqlError::expected(
            "Your statement must start with CREATE, DROP, SELECT, INSERT, UPDATE, ALTER, \
             EXECUTE, DELETE, GRANT, REVOKE, BEGIN, COMMIT, ROLLBACK, SAVEPOINT, SET, DEBUG or a \
             script statement.",
            first.pos,
            "",
        ))
    }

    /// The error of a `CREATE`, `DROP` or `ALTER` (`first`, not yet read)
    /// that none of the statements above takes: the original's syntax error for what it has no such
    /// statement of (`CREATE USER`, `DROP DATABASE`, `ALTER VIEW`, ...), which
    /// words and places itself differently for each of the three.
    fn other_ddl(&self, first: &Token) -> SqlError {
        let verb = first.tok.text().to_ascii_uppercase();
        let second = self.tokens.get(self.i + 1);
        let word = match second {
            Some(Token {
                tok: Tok::Ident(word),
                ..
            }) => word.to_ascii_uppercase(),
            _ => String::new(),
        };
        let looking = " Looking for TABLE, INDEX, VIEW, TRIGGER, PROCEDURE, FUNCTION or PACKAGE \
                       after the keyword";
        match verb.as_str() {
            "CREATE" => {
                let at = second.map_or(first.pos, |t| t.pos);
                let what = if word.is_empty() { "<keyword>" } else { "" };
                SqlError::unexpected(&format!("{what}{looking} CREATE."), at)
            }
            "DROP" => {
                // The word after the second is where it looks, or where a
                // word that is none starts.
                let at = match (second, self.tokens.get(self.i + 2)) {
                    (Some(_), Some(third)) if !word.is_empty() => third.pos,
                    (Some(second), _) if word.is_empty() => second.pos,
                    _ => self.end,
                };
                SqlError::unexpected(&format!("{looking} DROP."), at)
            }
            _ => SqlError::expected(
                " The keyword TABLE, FUNCTION, PROCEDURE or TRIGGER must follow the keyword \
                 ALTER ",
                second.map_or(first.pos, |t| t.pos),
                "",
            ),
        }
    }

    /// A statement on the transaction, if one starts here: `BEGIN
    /// TRAN[SACTION]`, `COMMIT [WORK]`, `ROLLBACK [WORK] [TO [SAVEPOINT]
    /// name]`, `SAVEPOINT name`, `SET TRAN[SACTION] mode`.
    fn transaction(&mut self) -> Result<Option<Transaction>, SqlError> {
        let tran = |p: &Parser, ahead: usize| {
            p.at_word_at(ahead, "TRAN") || p.at_word_at(ahead, "TRANSACTION")
        };
        if self.at_word("BEGIN") && tran(self, 1) {
            self.i += 2;
            return Ok(Some(Transaction::Begin));
        }
        if self.eat_word("COMMIT") {
            self.eat_word("WORK");
            return Ok(Some(Transaction::Commit));
        }
        if self.eat_word("ROLLBACK") {
            self.eat_word("WORK");
            if !self.eat_word("TO") {
                return Ok(Some(Transaction::Rollback(None)));
            }
            self.eat_word("SAVEPOINT");
            let name = self.name().ok_or_else(|| self.expected("savepoint name"))?;
            return Ok(Some(Transaction::Rollback(Some(name))));
        }
        if self.eat_word("SAVEPOINT") {
            let name = self.name().ok_or_else(|| self.expected("savepoint name"))?;
            return Ok(Some(Transaction::Savepoint(name)));
        }
        if self.at_word("SET") && tran(self, 1) {
            self.i += 2;
            let mode = self.name().ok_or_else(|| self.expected("AUTOCOMMIT_ON"))?;
            return Ok(Some(Transaction::Set(mode.to_ascii_uppercase())));
        }
        Ok(None)
    }

    /// Whether the token `ahead` places on is the word `word`.
    fn at_word_at(&self, ahead: usize, word: &str) -> bool {
        matches!(self.peek_at(ahead), Some(Tok::Ident(w)) if w.eq_ignore_ascii_case(word))
    }

    /// `CREATE VIEW name AS SELECT …`, after `VIEW`.
    fn create_view(&mut self) -> Result<Statement, SqlError> {
        let pos = self.grant_pos();
        let name = self.grant_name().ok_or_else(|| {
            SqlError::expected_explained(
                "<identifier>",
                " You are missing the view name after the keyword VIEW.",
                pos,
            )
        })?;
        if !self.eat_word("AS") {
            return Err(SqlError::expected_explained(
                "AS",
                " You are missing the AS keyword in CREATE VIEW after the view name.",
                self.grant_pos(),
            ));
        }
        if !self.at_word("SELECT") {
            return Err(SqlError::expected_explained(
                "SELECT",
                NEEDS_SELECT,
                self.grant_pos(),
            ));
        }
        let start = self.i;
        self.i += 1;
        let select = self.select().map_err(in_view_select)?;
        let tokens = &self.tokens[start..self.i];
        match view_order(tokens, &select) {
            Some(ViewOrder::Top(pos)) => return Err(SqlError::unexpected(NEEDS_TOP, pos)),
            Some(ViewOrder::Nested(pos, why)) => {
                return Err(in_view_select(SqlError::expected_explained(")", &why, pos)));
            }
            None => {}
        }
        Ok(Statement::CreateView(Box::new(CreateView {
            name: Named { name, pos },
            text: view_text(tokens, &self.source),
            select,
        })))
    }

    /// `CREATE DATABASE`, after `DATABASE`. What tflip does not write is
    /// refused where it is given: a password for the administrator, an
    /// encrypted dictionary and the AES types.
    fn create_database(&mut self) -> Result<Statement, SqlError> {
        let name = match self.peek().map(|t| &t.tok) {
            Some(Tok::Ident(_) | Tok::Quoted(_)) => self.name(),
            _ => None,
        }
        .ok_or_else(|| {
            SqlError::expected_explained(
                "<identifier>",
                " You are missing the database name after the word DATABASE.",
                self.grant_pos(),
            )
        })?;
        let mut description = String::new();
        let mut aes = None;
        let mut dd_password = None;
        let mut encrypt = None;
        while let Some(token) = self.peek().cloned() {
            if matches!(token.tok, Tok::Punct(";")) {
                break;
            }
            // The text after a keyword: a string, or the error that says it
            // is missing, at the keyword.
            let string = |parser: &mut Parser, what: &str, why: &str| {
                parser.i += 1;
                let at = parser.grant_pos();
                match parser.peek().map(|t| &t.tok) {
                    Some(Tok::Str(text)) => {
                        let text = text.clone();
                        parser.i += 1;
                        Ok(text)
                    }
                    _ => Err(SqlError::expected_explained(what, why, at)),
                }
            };
            if token.tok.is("DESCRIPTION") {
                description = string(
                    self,
                    "",
                    " You are missing the database description after the keyword DESCRIPTION.",
                )?;
            } else if token.tok.is("PASSWORD") {
                let password = string(
                    self,
                    "<string>",
                    " You are missing the database password after the keyword PASSWORD.",
                )?;
                if !password.is_empty() {
                    return Err(SqlError::unsupported(
                        "CREATE DATABASE with a PASSWORD",
                        token.pos,
                    ));
                }
            } else if token.tok.is("DDPASSWORD") {
                dd_password = Some(string(
                    self,
                    "<string>",
                    " You are missing the dictionary password for AES encryption after the \
                     keyword DDPassword.",
                )?);
            } else if token.tok.is("ENCRYPT") {
                self.i += 1;
                let flag = self.peek().map(|t| &t.tok);
                if flag.is_some_and(|t| t.is("TRUE")) {
                    encrypt = Some(token.pos);
                } else if !flag.is_some_and(|t| t.is("FALSE")) {
                    return Err(SqlError::expected_explained(
                        "",
                        " You are missing the encrypt flag (TRUE | FALSE) after the word ENCRYPT.",
                        self.pos(),
                    ));
                }
                self.i += 1;
            } else if token.tok.is("ENCRYPTIONTYPE") {
                self.i += 1;
                let why = " You are missing the encryption type (AES128 | AES256) after the \
                          keyword ENCRYPTIONTYPE.";
                match self.peek().map(|t| &t.tok) {
                    Some(t) if t.is("RC4") => aes = None,
                    Some(t) if t.is("AES128") => aes = Some(5),
                    Some(t) if t.is("AES256") => aes = Some(6),
                    Some(_) => return Err(SqlError::expected_explained("", why, self.pos())),
                    None => {
                        return Err(SqlError::expected_explained(
                            "<keyword>",
                            why,
                            self.grant_pos(),
                        ));
                    }
                }
                self.i += 1;
            } else {
                return Err(SqlError::invalid_create(&token.tok.text(), token.pos));
            }
        }
        let aes = match (aes, dd_password) {
            (None, _) => None,
            (Some(_), None) => {
                return Err(SqlError::invalid_create_bare(
                    "A dictionary password is required when using AES encryption.",
                ));
            }
            (Some(kind), Some(password)) => Some((kind, password)),
        };
        Ok(Statement::CreateDatabase(Box::new(CreateDatabase {
            name,
            description,
            aes,
            encrypt: encrypt.is_some(),
        })))
    }

    /// `CREATE PROCEDURE` or `ALTER PROCEDURE`, after `PROCEDURE`.
    fn create_procedure(&mut self, alter: bool) -> Result<Statement, SqlError> {
        let missing = |what: &str, pos: Pos| {
            SqlError::expected_explained(
                what,
                " -- Missing the procedure name after the keyword PROCEDURE",
                pos,
            )
        };
        let pos = self.grant_pos();
        let name = match self.peek().map(|t| &t.tok) {
            Some(Tok::Quoted(_)) => {
                return Err(missing("Identifier name should not be quoted.", pos));
            }
            _ => self.grant_name().ok_or_else(|| {
                missing(&format!("<identifier> but found: {}", self.found()), pos)
            })?,
        };
        if !self.eat_punct("(") {
            return Err(SqlError::expected_explained(
                "(",
                " -- Missing the '(' after the procedure name in CREATE PROCEDURE.",
                self.grant_pos(),
            ));
        }
        let mut params = Vec::new();
        let mut varying = None;
        let mut stand_alone = None;
        if !self.eat_punct(")") {
            loop {
                let pos = self.pos();
                if self.eat_word("CURSOR") {
                    self.expect_word("VARYING")?;
                    if !self.eat_word("OUTPUT") {
                        // The original looks past a `)` for it.
                        let at = self.i + usize::from(self.at_punct(")"));
                        let pos = self.tokens.get(at).map_or(self.end, |t| t.pos);
                        return Err(SqlError::expected("OUTPUT", pos, ""));
                    }
                    if varying.replace(pos).is_some() {
                        return Err(SqlError::invalid_create(
                            "Only one CURSOR VARYING OUTPUT can be specified. ",
                            pos,
                        ));
                    }
                } else {
                    let param = self.proc_param()?;
                    if param.output && stand_alone.is_none() {
                        stand_alone = Some(pos);
                    }
                    if !param.output
                        && params
                            .iter()
                            .any(|p: &ProcParam| !p.output && p.column.name == param.column.name)
                    {
                        return Err(SqlError::declaration(
                            &format!("Input parameter name already used: {}", param.column.name),
                            param.column.name_pos,
                        ));
                    }
                    params.push(param);
                }
                if self.eat_punct(")") {
                    break;
                }
                if !self.eat_punct(",") {
                    return Err(SqlError::expected_explained(
                        ")",
                        " You are missing the closing ')' in CREATE PROCEDURE after the \
                         parameter descriptions.",
                        self.grant_pos(),
                    ));
                }
            }
        }
        if let (Some(_), Some(pos)) = (varying, stand_alone) {
            return Err(SqlError::invalid_create(
                " Stand alone output parameter is not allowed with varying output cursor.",
                pos,
            ));
        }
        let body = match self.at_word("BEGIN") {
            true => self.procedure_script()?,
            false => self.procedure_function(varying.is_some())?,
        };
        Ok(Statement::CreateProcedure(Box::new(CreateProcedure {
            alter,
            name: Named { name, pos },
            params,
            varying: varying.is_some(),
            body,
        })))
    }

    /// A function's name, with its package's before a `.`; `why` is what a
    /// name missing is explained with (`fixtures/sql/ddfuncs`).
    fn function_name(&mut self, why: &str) -> Result<(Option<Named>, Named), SqlError> {
        let first = self.ddl_name(why)?;
        if !self.eat_punct(".") {
            return Ok((None, first));
        }
        let name = self.ddl_name(" -- Missing function name after package name.")?;
        Ok((Some(first), name))
    }

    /// A name that is no reserved word, and not quoted.
    fn ddl_name(&mut self, why: &str) -> Result<Named, SqlError> {
        let pos = self.grant_pos();
        if let Some(Tok::Quoted(_)) = self.peek().map(|t| &t.tok) {
            return Err(SqlError::expected_explained(
                "Identifier name should not be quoted.",
                why,
                pos,
            ));
        }
        let name = self.grant_name().ok_or_else(|| {
            SqlError::expected_explained(
                &format!("<identifier> but found: {}", self.found()),
                why,
                pos,
            )
        })?;
        Ok(Named { name, pos })
    }

    /// `CREATE FUNCTION` or `ALTER FUNCTION`, after `FUNCTION`.
    fn create_function(&mut self, alter: bool) -> Result<CreateFunction, SqlError> {
        let (package, name) = self.function_name(" -- Missing function name.")?;
        let mut new_name = None;
        if alter && !self.at_punct("(") {
            let why = " -- Missing <new-name> or ( after function-name in ALTER FUNCTION.";
            let pos = self.grant_pos();
            let (new_package, new) = self.function_name(why)?;
            let moved = |why: &str| SqlError::invalid_function(why, pos);
            match (&package, new_package) {
                (None, Some(_)) => {
                    return Err(moved(
                        "ALTER FUNCTION cannot be used to move function into a package.",
                    ));
                }
                (Some(old), Some(new)) if !old.name.eq_ignore_ascii_case(&new.name) => {
                    return Err(moved(
                        "ALTER FUNCTION cannot be used to move function from one package to \
                         another package.",
                    ));
                }
                _ => {}
            }
            new_name = Some(new);
        }
        let pos = self.grant_pos();
        if !self.eat_punct("(") {
            return Err(SqlError::expected_explained(
                "(",
                " Missing the '(' after the function name in CREATE FUNCTION.",
                pos,
            ));
        }
        // A name that is a scalar function's is refused at the name of a new
        // function, and at the `(` after the new name of an altered one.
        let scalar = |name: &str, at: Pos| match is_scalar_function(name) {
            true => Err(SqlError::invalid_function(
                "The function name cannot be the same as an existing scalar function.",
                at,
            )),
            false => Ok(()),
        };
        match (&new_name, alter, &package) {
            (_, _, Some(_)) => {}
            (Some(new), _, None) => scalar(&new.name, pos)?,
            (None, false, None) => scalar(&name.name, name.pos)?,
            (None, true, None) => {}
        }
        let open = self.i;
        // Each parameter's name, type and sizes, checked after the result's.
        let mut params: Vec<(Named, String, Vec<u32>)> = Vec::new();
        if !self.eat_punct(")") {
            loop {
                let var = self.ddl_name(" -- Missing variable name.")?;
                if params
                    .iter()
                    .any(|(p, ..)| p.name.eq_ignore_ascii_case(&var.name))
                {
                    return Err(SqlError::declaration(
                        &format!("Variable name {} has already been declared.", var.name),
                        var.pos,
                    ));
                }
                let (ty, sizes) =
                    self.function_type(" -- Missing the data type after variable name.", var.pos)?;
                params.push((var, ty, sizes));
                if self.eat_punct(")") {
                    break;
                }
                if !self.eat_punct(",") {
                    return Err(SqlError::expected_explained(
                        "<symbol>",
                        " Expecting comma or ) in function definition.",
                        self.pos(),
                    ));
                }
            }
        }
        let from = self.tokens[open - 1].end;
        let to = self.tokens[self.i - 1].pos;
        let text: String = self.source[from.min(to)..to].iter().collect();
        let pos = self.grant_pos();
        if !self.eat_word("RETURNS") {
            return Err(SqlError::expected_explained(
                "RETURNS",
                ". Expecting return data type.",
                pos,
            ));
        }
        let type_pos = self.grant_pos();
        let (ty, sizes) = self.function_type(" -- Expecting data type identifier.", type_pos)?;
        if ty.eq_ignore_ascii_case("CURSOR") {
            return Err(SqlError::invalid_function(
                "Cursor is not a valid output parameter type.",
                type_pos,
            ));
        }
        crate::script::check_declared_type(&ty, &sizes, type_pos)?;
        let returns = spelled_type(&ty, &sizes);
        let mut description = None;
        if self.eat_word("DESCRIPTION") {
            let Some(Tok::Str(text)) = self.peek().map(|t| &t.tok) else {
                return Err(SqlError::expected_explained(
                    "",
                    " You are missing the function description after the keyword DESCRIPTION.",
                    self.pos(),
                ));
            };
            description = Some(text.clone());
            self.i += 1;
        }
        if !self.at_word("BEGIN") {
            return Err(SqlError::expected_explained("BEGIN", "", self.grant_pos()));
        }
        let ProcBody::Script(body) = self.procedure_script()? else {
            unreachable!("a script body");
        };
        // The parameters' types are checked when the rest is known to parse.
        for (var, ty, sizes) in &params {
            if ty.eq_ignore_ascii_case("CURSOR") {
                return Err(SqlError::invalid_function(
                    "Cursor is not a valid input parameter type.",
                    var.pos,
                ));
            }
            crate::script::check_declared_type(ty, sizes, var.pos)?;
        }
        Ok(CreateFunction {
            alter,
            package,
            name,
            new_name,
            params: text.trim().to_owned(),
            param_count: params.len(),
            returns,
            description,
            body,
        })
    }

    /// The type a function's parameter or result is declared with: its name
    /// and sizes; `why` explains a name that is missing, which is told at
    /// `pos`.
    fn function_type(&mut self, why: &str, pos: Pos) -> Result<(String, Vec<u32>), SqlError> {
        // `VARCHAR` and `CURSOR` are among the reserved words, and types.
        let reserved = match self.peek().map(|t| &t.tok) {
            Some(Tok::Ident(word)) => {
                let word = word.to_ascii_uppercase();
                word != "VARCHAR" && word != "CURSOR" && KEYWORDS.contains(&word.as_str())
            }
            _ => false,
        };
        let name = match reserved {
            true => None,
            false => self.name(),
        };
        let Some(name) = name else {
            return Err(SqlError::expected_explained(
                &format!("<identifier> but found: {}", self.found()),
                why,
                pos,
            ));
        };
        let sizes = self.type_sizes()?;
        Ok((name, sizes))
    }

    /// `CREATE PACKAGE name [DESCRIPTION text]`, after `PACKAGE`.
    fn create_package(&mut self) -> Result<Statement, SqlError> {
        let missing = |what: &str, pos: Pos| {
            SqlError::expected_explained(
                what,
                " You are missing the package name after the keyword PACKAGE.",
                pos,
            )
        };
        let pos = self.pos();
        let name = match self.peek().map(|t| &t.tok) {
            Some(Tok::Quoted(_)) => {
                return Err(missing("Identifier name should not be quoted.", pos));
            }
            _ => self.grant_name().ok_or_else(|| {
                missing(&format!("<identifier> but found: {}", self.found()), pos)
            })?,
        };
        let mut description = None;
        if self.eat_word("DESCRIPTION") {
            let Some(Tok::Str(text)) = self.peek().map(|t| &t.tok) else {
                return Err(SqlError::expected_explained(
                    "",
                    " You are missing the package description after the keyword DESCRIPTION.",
                    self.pos(),
                ));
            };
            description = Some(text.clone());
            self.i += 1;
        }
        Ok(Statement::FunctionDdl(Box::new(
            FunctionDdl::CreatePackage {
                name: Named { name, pos },
                description,
            },
        )))
    }

    /// The next token as an error says what it found: its text, or nothing
    /// at the end.
    fn found(&self) -> String {
        self.found_or("")
    }

    /// [`Parser::found`], with `end` for the end of the text.
    fn found_or(&self, end: &str) -> String {
        self.peek().map_or_else(|| end.to_owned(), |t| t.tok.text())
    }

    /// A parameter of a procedure: `name type [(n[, m])] [OUTPUT]`.
    fn proc_param(&mut self) -> Result<ProcParam, SqlError> {
        let name_pos = self.pos();
        let name = self.grant_name().ok_or_else(|| {
            SqlError::expected_explained(
                &format!("<identifier> but found: {}", self.found()),
                "",
                name_pos,
            )
        })?;
        let type_pos = self.pos();
        let Some(type_name) = self.name() else {
            return Err(SqlError::expected_explained(
                &format!("<identifier> but found: {}", self.found()),
                " You are missing the new field type after your field name.",
                type_pos,
            ));
        };
        let mut params = Vec::new();
        if self.eat_punct("(") {
            loop {
                params.push(self.integer()?);
                if self.eat_punct(")") {
                    break;
                }
                if params.len() == 2 && self.at_punct(",") {
                    return Err(self.expected_here(") missing close ')' in create"));
                }
                self.expect_punct(",")?;
            }
        }
        let output = self.eat_word("OUTPUT");
        Ok(ProcParam {
            column: ColumnDef {
                name,
                name_pos,
                type_name,
                type_pos,
                params,
                compressed: false,
                nullable: None,
                nocptrans: false,
                rules: FieldRules::default(),
            },
            output,
        })
    }

    /// `BEGIN script END` of a procedure, at `BEGIN`.
    fn procedure_script(&mut self) -> Result<ProcBody, SqlError> {
        let begin = self.i;
        self.i += 1;
        // A statement not followed by `;` is a token where the original
        // expects one, `END` among them.
        self.stored = true;
        let script = self.script();
        self.stored = false;
        script.map_err(|error| match self.peek() {
            Some(token) if error.native == 2115 && error.message.contains("found: ;") => {
                SqlError::unexpected_token(&token.tok.unexpected_text(), token.pos)
            }
            _ => error,
        })?;
        let empty = self.i == begin + 1;
        if !self.at_word("END") {
            // A statement that is none, such as an `ENDIF` nothing opened.
            if self.peek().is_some() {
                self.statement()?;
            }
            return Err(SqlError::script_end(self.end));
        }
        if empty {
            return Err(SqlError::expected(
                "<SQL Script Statements>",
                self.tokens[self.i].pos,
                "",
            ));
        }
        // The text between them as written, but the first line break, with
        // the blanks before it (`fixtures/sql/ddprocs`).
        let from = self.tokens[begin].end;
        let to = self.tokens[self.i].pos;
        self.i += 1;
        let mut text: &[char] = &self.source[from.min(to)..to];
        let blanks = text.iter().take_while(|c| matches!(c, ' ' | '\t')).count();
        match text.get(blanks..) {
            Some(['\r', '\n', ..]) => text = &text[blanks + 2..],
            Some(['\r' | '\n', ..]) => text = &text[blanks + 1..],
            _ => {}
        }
        Ok(ProcBody::Script(text.iter().collect()))
    }

    /// `FUNCTION name IN [COM]LIBRARY file` of a procedure, at the word
    /// after the parameters.
    fn procedure_function(&mut self, varying: bool) -> Result<ProcBody, SqlError> {
        if !self.eat_word("FUNCTION") {
            return Err(SqlError::expected_explained(
                "FUNCTION",
                " You are missing the keyword FUNCTION in CREATE PROCEDURE after the parameter \
                 descriptions.",
                self.grant_pos(),
            ));
        }
        let pos = self.grant_pos();
        let function = match self.peek().map(|t| &t.tok) {
            Some(Tok::Str(_)) => None,
            _ => self.grant_name(),
        }
        .ok_or_else(|| {
            SqlError::expected_explained(
                "<identifier>",
                " You are missing the DLL function name after the keyword FUNCTION.",
                pos,
            )
        })?;
        if !self.eat_word("IN") {
            return Err(SqlError::expected_explained(
                "IN",
                " You are missing the keyword IN after the dll function name.",
                self.grant_pos(),
            ));
        }
        let keyword = " You are missing the keyword LIBRARY or COMLIBRARY after the keyword IN \
                       in CREATE PROCEDURE";
        let com = match (self.eat_word("LIBRARY"), self.eat_word("COMLIBRARY")) {
            (false, false) if self.peek().is_none() => {
                return Err(SqlError::expected_explained(
                    "<keyword>",
                    keyword,
                    self.grant_pos(),
                ));
            }
            (false, false) => {
                // At the `IN`, not at what follows it.
                let pos = self.tokens[self.i - 1].pos;
                return Err(SqlError {
                    state: "42000".to_owned(),
                    native: 2117,
                    message: format!(
                        "Unexpected token: <keyword>{keyword}{} You are missing the key LIBRARY \
                         or COMLIBRARY after the keyword IN in CREATE PROCEDURE",
                        crate::error::at(pos)
                    ),
                });
            }
            (_, com) => com,
        };
        let pos = self.grant_pos();
        let library = match self.peek().map(|t| &t.tok) {
            Some(Tok::Ident(_) | Tok::Quoted(_)) => self.name(),
            _ => None,
        }
        .ok_or_else(|| {
            SqlError::expected_explained(
                "<identifier>",
                " You are missing DLL file name after the keyword LIBRARY.",
                pos,
            )
        })?;
        if varying {
            return Err(SqlError {
                state: "HY000".to_owned(),
                native: 2215,
                message: "Invalid create options:  Varying output stored procedure is only \
                          supported using SQL script."
                    .to_owned(),
            });
        }
        Ok(ProcBody::External {
            function,
            library,
            com,
        })
    }

    /// `CREATE TABLE` or `CREATE [ONLINE] [UNIQUE] INDEX`, after `CREATE`.
    fn create(&mut self) -> Result<Statement, SqlError> {
        if self.eat_word("TABLE") {
            return Ok(Statement::CreateTable(Box::new(self.create_table()?)));
        }
        let start = self.pos();
        let online = self.eat_word("ONLINE");
        let unique = self.eat_word("UNIQUE");
        if !self.at_word("INDEX") && (online || unique) {
            if self.peek().is_none() {
                return Err(SqlError::unexpected(
                    "<keyword> Looking for TABLE, INDEX, VIEW, TRIGGER, PROCEDURE, FUNCTION or \
                     PACKAGE after the keyword CREATE.",
                    start,
                ));
            }
            let word = if unique { "UNIQUE" } else { "ONLINE" };
            return Err(SqlError::expected(
                &format!(" {word} keyword is only valid for creating index."),
                self.pos(),
                "",
            ));
        }
        self.expect_word("INDEX")?;
        let pos = self.pos();
        let name = self.grant_name().ok_or_else(|| {
            self.expected_here(
                "<identifier> You are missing the index name after the keyword INDEX.",
            )
        })?;
        let name = Named { name, pos };
        if !self.eat_word("ON") {
            return Err(
                self.expected_here("ON You are missing the keyword ON after the index name.")
            );
        }
        let table = self.created_target(
            " missing table name You are missing the table name after the keyword ON.",
            " You are missing the column list after the table name.",
        )?;
        self.expect_punct_explained(
            "(",
            " You are missing the column list after the table name.",
        )?;
        let mut columns = Vec::new();
        loop {
            let pos = self.pos();
            let column = self.name().ok_or_else(|| {
                self.expected_here(
                    "<identifier> missing column name. You need to put column identifiers \
                     between the \"( )\".",
                )
            })?;
            let descending = match self.eat_word("DESC") {
                true => true,
                false => {
                    self.eat_word("ASC");
                    false
                }
            };
            columns.push((Named { name: column, pos }, descending));
            if self.eat_punct(")") {
                break;
            }
            if !self.eat_punct(",") {
                return Err(self.expected_here(
                    ") You are missing the closing ')' after your column identifiers in CREATE \
                     INDEX.",
                ));
            }
        }
        let mut create = CreateIndex {
            unique,
            online,
            candidate: false,
            not_deleted: false,
            file: None,
            page_size: None,
            content: None,
            full_text: Default::default(),
            name,
            table,
            columns,
        };
        self.index_options(&mut create)?;
        Ok(Statement::CreateIndex(Box::new(create)))
    }

    /// The options after the columns of `CREATE INDEX`: `CANDIDATE`,
    /// `NOTDELETED`, `IN FILE name`, `PAGESIZE n`, and `CONTENT` with the
    /// options of a full-text index (refused without it, as by the original
    /// engine).
    fn index_options(&mut self, create: &mut CreateIndex) -> Result<(), SqlError> {
        let mut full_text = false;
        loop {
            let pos = self.pos();
            let Some(Tok::Ident(word)) = self.peek_at(0) else {
                break;
            };
            let word = word.to_ascii_uppercase();
            self.i += 1;
            match word.as_str() {
                "CANDIDATE" => {
                    if std::mem::replace(&mut create.candidate, true) {
                        return Err(SqlError::invalid_create(
                            "The CANDIDATE option has already been specified.",
                            pos,
                        ));
                    }
                }
                "NOTDELETED" => create.not_deleted = true,
                "CONTENT" => create.content = Some(pos),
                "IN" => {
                    if create.file.is_some() {
                        return Err(SqlError::invalid_create(
                            "The IN FILE option has already been specified.",
                            pos,
                        ));
                    }
                    self.expect_word("FILE")?;
                    let file = match self.peek().map(|t| &t.tok) {
                        Some(Tok::Ident(name) | Tok::Quoted(name)) => name.clone(),
                        _ => {
                            return Err(SqlError::expected(
                                "<identifier> You are missing the file name after the keyword \
                                 FILE.",
                                self.grant_pos(),
                                "",
                            ));
                        }
                    };
                    self.i += 1;
                    create.file = Some(file);
                }
                "PAGESIZE" => {
                    let size = match self.peek().map(|t| &t.tok) {
                        Some(Tok::Number(n)) if n.bytes().all(|b| b.is_ascii_digit()) => {
                            n.parse().unwrap_or(u64::MAX)
                        }
                        other => {
                            let text = match other {
                                Some(Tok::Str(_)) | None => String::new(),
                                Some(token) => token.text(),
                            };
                            return Err(SqlError::malformed_page_size(&text, self.grant_pos()));
                        }
                    };
                    self.i += 1;
                    create.page_size = Some(size);
                }
                "MIN" | "MAX" => {
                    full_text = true;
                    self.expect_word("WORD")?;
                    let n = self.integer()?;
                    match word.as_str() {
                        "MIN" => create.full_text.min_word = Some(n),
                        _ => create.full_text.max_word = Some(n),
                    }
                }
                "NEW" | "DELIMITERS" | "DROPCHARS" | "NOISE" | "CONDITIONALS" => {
                    full_text = true;
                    let new = word == "NEW";
                    let which = match new {
                        true => self.name().unwrap_or_default().to_ascii_uppercase(),
                        false => word.clone(),
                    };
                    let text = self.string().unwrap_or_default();
                    let options = &mut create.full_text;
                    let list = match which.as_str() {
                        "DELIMITERS" => &mut options.delimiters,
                        "DROPCHARS" => &mut options.drop,
                        "NOISE" => &mut options.noise,
                        _ => &mut options.conditionals,
                    };
                    *list = Some((new, text));
                }
                "NOTMAINTAINED" => (full_text, create.full_text.fixed) = (true, true),
                "CASESENSITIVE" => (full_text, create.full_text.case_sensitive) = (true, true),
                "KEEPSCORE" => (full_text, create.full_text.keep_score) = (true, true),
                "PROTECTNUMBERS" => {
                    (full_text, create.full_text.protect_numbers) = (true, true);
                }
                _ => {
                    return Err(SqlError::invalid_create(
                        &self.tokens[self.i - 1].tok.text(),
                        pos,
                    ));
                }
            }
        }
        let full_text_index = create.content.is_some();
        if full_text && !full_text_index || full_text_index && (create.unique || create.candidate) {
            return Err(SqlError::full_text_option());
        }
        if create.unique && create.candidate {
            return Err(SqlError::invalid_create_bare(
                "Index cannot be both unique and candidate.",
            ));
        }
        // The original engine reads the error of a name that goes on with a
        // `.` from the character after it.
        if self.at_punct(".") {
            return Err(SqlError::unexpected_token(".", self.pos()));
        }
        Ok(())
    }

    /// `CREATE TRIGGER`, after `TRIGGER`: `name ON table timing event BEGIN
    /// script END`, then `NO MEMOS`, `NO TRANSACTION` and `PRIORITY n` in
    /// any order. The script is checked here, and kept as written.
    fn create_trigger(&mut self, alter: bool) -> Result<CreateTrigger, SqlError> {
        // Where the original puts a missing word at the end of the text: at
        // the last word.
        let here = |p: &Parser| match p.tokens.get(p.i) {
            Some(token) => token.pos,
            None => p.tokens.last().map_or(1, |token| token.pos),
        };
        let pos = self.pos();
        let name = self.name().ok_or_else(|| {
            SqlError::expected_explained(
                "<identifier>",
                " You are missing the trigger name after the keyword TRIGGER",
                here(self),
            )
        })?;
        let name = Named { name, pos };
        if !self.eat_word("ON") {
            return Err(SqlError::expected_explained(
                "ON",
                " You are missing the keyword ON after the trigger name.",
                here(self),
            ));
        }
        let pos = self.pos();
        let table = self.name().ok_or_else(|| self.expected("<identifier>"))?;
        let table = Named { name: table, pos };
        let timing = if self.eat_word("BEFORE") {
            1
        } else if self.eat_word("AFTER") {
            4
        } else if self.eat_word("INSTEAD") {
            self.expect_word("OF")?;
            2
        } else if self.eat_word("CONFLICT") {
            self.eat_word("ON");
            8
        } else {
            return Err(self.expected("BEFORE"));
        };
        // The database's events, which a trigger on `DATABASE` has and one on
        // a table has not.
        let database_event = [
            ("OPEN_TABLE", 4),
            ("CLOSE_TABLE", 5),
            ("CONNECT", 6),
            ("DISCONNECT", 7),
        ]
        .into_iter()
        .find(|(word, _)| self.at_word(word));
        let event = if let Some((_, event)) = database_event {
            if !table.name.eq_ignore_ascii_case("DATABASE") {
                return Err(SqlError::expected_explained(
                    "",
                    " The CREATE TRIGGER statement must specify ON DATABASE for the specified \
                     event type.",
                    self.pos(),
                ));
            }
            self.i += 1;
            event
        } else if self.eat_word("INSERT") {
            1
        } else if self.eat_word("UPDATE") {
            2
        } else if self.eat_word("DELETE") {
            3
        } else {
            return Err(SqlError::expected_explained(
                "",
                " You are missing the trigger event type (such as INSERT, UPDATE, DELETE) in \
                 CREATE TRIGGER after the trigger type.",
                self.pos(),
            ));
        };
        if self.at_word("FUNCTION") {
            return self.trigger_library(name, table, timing, event, alter);
        }
        if !self.at_word("BEGIN") {
            return Err(match self.tokens.get(self.i) {
                Some(token) => {
                    let mut error = SqlError::unexpected(
                        "<SQL Script Statements>",
                        token.pos + token.tok.text().chars().count(),
                    );
                    error.message.push_str(
                        " You are missing the keyword FUNCTION or BEGIN in CREATE TRIGGER after \
                         the event type.",
                    );
                    error
                }
                None => self.expected("BEGIN"),
            });
        }
        let begin = self.i;
        // The options at the end, read back from it, and the `END` before
        // them.
        let mut tail = self.tokens.len();
        if matches!(
            self.tokens.last(),
            Some(Token {
                tok: Tok::Punct(";"),
                ..
            })
        ) {
            tail -= 1;
        }
        let mut last = tail;
        let (mut options, mut priority) = (3u32, 1u32);
        loop {
            let word = |at: usize, w: &str| at > begin && self.tokens[at].tok.is(w);
            if last >= 2 && word(last - 2, "NO") && word(last - 1, "VALUES") {
                options = 0;
                last -= 2;
            } else if last >= 2 && word(last - 2, "NO") && word(last - 1, "MEMOS") {
                options &= !2;
                last -= 2;
            } else if last >= 2 && word(last - 2, "NO") && word(last - 1, "TRANSACTION") {
                options |= 4;
                last -= 2;
            } else if last >= 2
                && word(last - 2, "PRIORITY")
                && let Tok::Number(n) = &self.tokens[last - 1].tok
            {
                priority = n.parse().unwrap_or(0);
                last -= 2;
            } else {
                break;
            }
        }
        if last <= begin + 1 || !self.tokens[last - 1].tok.is("END") {
            self.i = last;
            return Err(self.expected("END"));
        }
        let end = last - 1;
        // The script, checked as a trigger body is run.
        let rest = self.tokens.split_off(end);
        self.i = begin + 1;
        self.stored = true;
        let checked = self.script();
        self.stored = false;
        let all = self.peek().is_none();
        let at = self.pos();
        let empty = self.i == begin + 1;
        self.tokens.extend(rest);
        checked?;
        if !all {
            return Err(SqlError::expected("END", at, ""));
        }
        if empty {
            return Err(SqlError::expected(
                "<SQL Script Statements>",
                self.tokens[end].pos,
                "",
            ));
        }
        let from = self.tokens[begin].pos + "BEGIN".len();
        let to = self.tokens[end].pos;
        let body: String = self.source[from.min(to)..to].iter().collect();
        self.i = tail;
        Ok(CreateTrigger {
            name,
            table,
            timing,
            event,
            body,
            library: None,
            options,
            priority,
            alter,
        })
    }

    /// `FUNCTION f IN LIBRARY name` and the options after it, of a trigger
    /// that is a function of a library, at the word `FUNCTION`.
    fn trigger_library(
        &mut self,
        name: Named,
        table: Named,
        timing: u32,
        event: u32,
        alter: bool,
    ) -> Result<CreateTrigger, SqlError> {
        self.i += 1;
        let function = self
            .grant_name()
            .ok_or_else(|| self.expected("<identifier>"))?;
        self.expect_word("IN")?;
        let assembly = if self.eat_word("ASSEMBLY") {
            true
        } else if self.eat_word("LIBRARY") {
            false
        } else {
            return Err(self.expected("LIBRARY"));
        };
        let library = match self.peek().map(|t| &t.tok) {
            Some(Tok::Ident(_) | Tok::Quoted(_)) => self.name(),
            _ => None,
        }
        .ok_or_else(|| self.expected("<identifier>"))?;
        let (mut options, mut priority) = (3u32, 1u32);
        loop {
            if self.eat_word("NO") {
                if self.eat_word("VALUES") {
                    options = 0;
                } else if self.eat_word("MEMOS") {
                    options &= !2;
                } else {
                    self.expect_word("TRANSACTION")?;
                    options |= 4;
                }
            } else if self.eat_word("PRIORITY") {
                match self.peek().map(|t| &t.tok) {
                    Some(Tok::Number(n)) => {
                        priority = n.parse().unwrap_or(0);
                        self.i += 1;
                    }
                    _ => return Err(self.expected("<number>")),
                }
            } else {
                break;
            }
        }
        self.eat_punct(";");
        Ok(CreateTrigger {
            name,
            table,
            timing,
            event,
            body: String::new(),
            library: Some(TriggerLibrary {
                function,
                library,
                assembly,
            }),
            options,
            priority,
            alter,
        })
    }

    /// `CREATE TABLE`, after `TABLE`.
    fn create_table(&mut self) -> Result<CreateTable, SqlError> {
        let table = self.created_target(
            " You are missing the table name after the keyword TABLE.",
            " You are missing the column descriptors after the table name in CREATE TABLE.",
        )?;
        self.expect_punct_explained(
            "(",
            " You are missing the column descriptors after the table name in CREATE TABLE.",
        )?;
        if self.at_punct(")") {
            return Err(SqlError::invalid_create(
                "Missing column definitions in CREATE TABLE.",
                self.pos(),
            ));
        }
        let mut create = CreateTable {
            table,
            columns: Vec::new(),
            block_size: None,
            primary_key: None,
            in_database: None,
            free: false,
            ignore_transactions: false,
        };
        loop {
            if !self.table_constraint(&mut create)? {
                let column = self.created_column(&mut create)?;
                create.columns.push(column);
            }
            if self.eat_punct(")") {
                break;
            }
            if !self.eat_punct(",") {
                return Err(self.expected_here(
                    ") You are missing the closing ')' after the column identifiers in CREATE \
                     TABLE.",
                ));
            }
            // The original engine takes a comma before the `)`.
            if self.eat_punct(")") {
                break;
            }
        }
        loop {
            let pos = self.pos();
            if self.eat_word("MEMOBLOCKSIZE") {
                create.block_size = Some(self.integer()?);
            } else if self.eat_word("IN") {
                self.expect_word("DATABASE")?;
                create.in_database = Some(pos);
            } else if self.eat_word("AS") {
                self.expect_word("FREE")?;
                self.expect_word("TABLE")?;
                create.free = true;
            } else if self.eat_word("IGNORE") {
                // At the end of the text, the original engine places this
                // error at the last character.
                if !self.eat_word("TRANSACTIONS") {
                    let at = match self.peek() {
                        Some(token) => token.pos,
                        None => self.end,
                    };
                    return Err(SqlError::expected(
                        " Expecting keyword TRANSACTIONS after IGNORE.",
                        at,
                        "",
                    ));
                }
                create.ignore_transactions = true;
            } else {
                break;
            }
        }
        Ok(create)
    }

    /// A `PRIMARY KEY (columns)` or `CONSTRAINT name PRIMARY KEY (columns)`
    /// among the columns of `create`, which it goes to; false if the next
    /// element is a column.
    fn table_constraint(&mut self, create: &mut CreateTable) -> Result<bool, SqlError> {
        let name = if self.eat_word("CONSTRAINT") {
            let name = self.grant_name().ok_or_else(|| {
                self.expected("<identifier> No identifier found after the CONSTRAINT keyword.")
            })?;
            Some(name)
        } else if self.at_word("PRIMARY") {
            None
        } else {
            return Ok(false);
        };
        let pos = self.pos();
        if !self.eat_word("PRIMARY") {
            return Err(self.expected(
                "<keyword> No table constraint definition found after CONSTRAINT clause.",
            ));
        }
        self.expect_word("KEY")?;
        if create.primary_key.is_some() {
            return Err(SqlError::unexpected(
                "Only one PRIMARY KEY can exist per table.",
                pos,
            ));
        }
        self.expect_punct("(")?;
        let mut columns = Vec::new();
        loop {
            let pos = self.pos();
            let name = self.name().ok_or_else(|| self.expected("<identifier>"))?;
            let descending = self.eat_word("DESC");
            if !descending {
                self.eat_word("ASC");
            }
            columns.push((Named { name, pos }, descending));
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(")")?;
        let name = name.map(|name| name.to_ascii_uppercase());
        create.primary_key = Some(PrimaryKey { name, columns });
        Ok(true)
    }

    /// A column of `CREATE TABLE`: its name, type and options, a `PRIMARY
    /// KEY` among them going to `create`.
    fn created_column(&mut self, create: &mut CreateTable) -> Result<ColumnDef, SqlError> {
        let name_pos = self.pos();
        let name = self
            .grant_name()
            .ok_or_else(|| match self.peek().map(|t| &t.tok) {
                Some(Tok::Ident(_)) => self.expected_here(
                    "<identifier> An invalid keyword was found in the CREATE TABLE column list.",
                ),
                // Placed at the comma or parenthesis before it.
                _ => SqlError::expected("<keyword>", self.tokens[self.i - 1].pos, ""),
            })?;
        let type_pos = self.pos();
        let type_name = self.column_type()?;
        let params = self.column_sizes()?;
        let compressed = self.eat_word("COMPRESSED");
        let mut rules = FieldRules::default();
        // A plain `NULL` or `NOT NULL`, which may be there once.
        let mut nullable = None;
        let mut nocptrans = false;
        loop {
            let pos = self.pos();
            if self.eat_word("NOCPTRANS") {
                if std::mem::replace(&mut nocptrans, true) {
                    return Err(SqlError::invalid_create(
                        " Redundant NOCPTRANS specification.",
                        pos,
                    ));
                }
            } else if self.eat_word("PRIMARY") {
                self.expect_word("KEY")?;
                let column = Named {
                    name: name.clone(),
                    pos: name_pos,
                };
                Self::column_primary_key(create, None, column, pos)?;
            } else if self.at_word("NOT") || self.at_word("NULL") {
                let not = self.eat_word("NOT");
                if not {
                    self.expect_word("NULL")?;
                } else {
                    self.i += 1;
                }
                if nullable.replace(!not).is_some() {
                    return Err(SqlError::invalid_create(
                        " Redundant NULL or NOT NULL specification.",
                        pos,
                    ));
                }
            } else if self.eat_word("DEFAULT") {
                let Some(value) = self.string() else {
                    return Err(SqlError::expected(
                        "<string> missing default field value after \"DEFAULT\" - expecting a \
                         string literal.",
                        pos,
                        "",
                    ));
                };
                if rules.default.replace(value).is_some() {
                    return Err(SqlError::invalid_create(
                        " Redundant DEFAULT value specification.",
                        pos,
                    ));
                }
            } else if self.eat_word("CONSTRAINT") {
                let column = Named {
                    name: name.clone(),
                    pos: name_pos,
                };
                self.column_constraint(create, &mut rules, column)?;
            } else if let Some(Tok::Ident(word)) = self.peek_at(0) {
                let why = format!("Unrecognized column option: {word}");
                return Err(SqlError::unexpected(&why, pos));
            } else {
                break;
            }
        }
        Ok(ColumnDef {
            name,
            name_pos,
            type_name,
            type_pos,
            params,
            compressed,
            nullable,
            nocptrans,
            rules,
        })
    }

    /// A column's constraint after `CONSTRAINT`: `NOT NULL`, `MINIMUM`,
    /// `MAXIMUM` or `ERROR MESSAGE` and its value into `rules`, or `name
    /// PRIMARY KEY` of `column` into `create`.
    fn column_constraint(
        &mut self,
        create: &mut CreateTable,
        rules: &mut FieldRules,
        column: Named,
    ) -> Result<(), SqlError> {
        let pos = self.pos();
        let redundant =
            |what: &str| SqlError::invalid_create(&format!(" Redundant {what} Constraint."), pos);
        if self.eat_word("NOT") {
            self.expect_word("NULL")?;
            return match std::mem::replace(&mut rules.not_null, true) {
                true => Err(redundant("NOT NULL")),
                false => Ok(()),
            };
        }
        // The original engine words a second MAXIMUM so.
        let (rule, what) = if self.eat_word("ERROR") {
            self.expect_word("MESSAGE")?;
            (&mut rules.message, "ERROR MESSAGE")
        } else if self.eat_word("MINIMUM") {
            (&mut rules.minimum, "MINIMUM")
        } else if self.eat_word("MAXIMUM") {
            (&mut rules.maximum, "MAXMUM")
        } else {
            let name = self
                .grant_name()
                .ok_or_else(|| self.expected("<identifier> or valid constraint keyword."))?;
            let at = self.pos();
            if !self.eat_word("PRIMARY") {
                return Err(SqlError::expected_explained(
                    "PRIMARY or <string literal>",
                    ". If defining a field constraint, the value must be within single quotes. \
                     Otherwise, the keyword PRIMARY is expected for a PRIMARY KEY clause.",
                    at,
                ));
            }
            self.expect_word("KEY")?;
            let name = Some(name.to_ascii_uppercase());
            return Self::column_primary_key(create, name, column, at);
        };
        let value = self.string().ok_or_else(|| self.expected("<string>"))?;
        match rule.replace(value) {
            Some(_) => Err(redundant(what)),
            None => Ok(()),
        }
    }

    /// The `PRIMARY KEY` (at `pos`) of `column`, named `name`, as the one
    /// primary key of `create`.
    fn column_primary_key(
        create: &mut CreateTable,
        name: Option<String>,
        column: Named,
        pos: Pos,
    ) -> Result<(), SqlError> {
        if create.primary_key.is_some() {
            return Err(SqlError::unexpected(
                "Only one PRIMARY KEY can exist per table.",
                pos,
            ));
        }
        create.primary_key = Some(PrimaryKey {
            name,
            columns: vec![(column, false)],
        });
        Ok(())
    }

    /// A string literal's text.
    fn string(&mut self) -> Option<String> {
        match self.peek().map(|t| &t.tok) {
            Some(Tok::Str(value)) => {
                let value = value.clone();
                self.i += 1;
                Some(value)
            }
            _ => None,
        }
    }

    /// The name of a column's type.
    fn column_type(&mut self) -> Result<String, SqlError> {
        self.name().ok_or_else(|| {
            SqlError::expected_explained(
                &format!("<identifier> but found: {}", self.found()),
                " You are missing the new field type after your field name.",
                self.grant_pos(),
            )
        })
    }

    /// The sizes in parentheses after a column's type, if any.
    fn column_sizes(&mut self) -> Result<Vec<u64>, SqlError> {
        let mut sizes = Vec::new();
        if !self.eat_punct("(") {
            return Ok(sizes);
        }
        loop {
            let first = sizes.is_empty();
            let (found, pos) = match self.peek() {
                Some(Token {
                    tok: Tok::Number(n),
                    ..
                }) if n.bytes().all(|b| b.is_ascii_digit()) => {
                    sizes.push(n.parse().unwrap_or(u64::MAX));
                    self.i += 1;
                    if self.eat_punct(")") {
                        return Ok(sizes);
                    }
                    // At most two sizes are read (`NUMERIC(5,2,1)` lacks its `)`).
                    if sizes.len() == 2 && self.at_punct(",") {
                        return Err(self.expected_here(") missing close ')' in create"));
                    }
                    if !self.eat_punct(",") {
                        return Err(self.expected_here("<symbol> expected comma or ')' in create"));
                    }
                    continue;
                }
                // The original engine reads the number up to its point, and
                // places the error at the point's digits.
                Some(Token {
                    tok: Tok::Number(n),
                    pos,
                    ..
                }) if n.contains('.') => {
                    let dot = n.find('.').unwrap_or(0);
                    (n[dot..].to_owned(), pos + dot + 1)
                }
                _ => (self.found(), self.grant_pos()),
            };
            return Err(SqlError::malformed_column_size(&found, first, pos));
        }
    }

    /// Passes what a column's constraint has that is not read, up to the
    /// next action of `ALTER TABLE`.
    fn skip_column_rest(&mut self) {
        while self.peek().is_some()
            && ![
                "ADD",
                "ALTER",
                "DROP",
                "POSITION",
                "MEMOBLOCKSIZE",
                "IGNORE",
                "RESPECT",
            ]
            .iter()
            .any(|w| self.at_word_at(0, w))
            && !self.at_punct(";")
        {
            self.i += 1;
        }
    }

    /// A column's name, type and options, as `ALTER TABLE` defines it; where
    /// its first constraint is goes to `constraint`.
    fn column_def(&mut self, constraint: &mut Option<Pos>) -> Result<ColumnDef, SqlError> {
        let name_pos = self.pos();
        let name = self
            .grant_name()
            .ok_or_else(|| self.expected_here("<identifier> You are missing the column names."))?;
        let type_pos = self.pos();
        let type_name = self.column_type()?;
        let params = self.column_sizes()?;
        let compressed = self.eat_word("COMPRESSED");
        let mut rules = FieldRules::default();
        let mut nullable = None;
        let mut nocptrans = false;
        loop {
            let pos = self.pos();
            if self.at_word("NOT") || self.at_word("NULL") {
                let not = self.eat_word("NOT");
                self.expect_word("NULL")?;
                if nullable.replace(!not).is_some() {
                    return Err(SqlError::invalid_create(
                        " Redundant NULL or NOT NULL specification.",
                        pos,
                    ));
                }
            } else if self.eat_word("NOCPTRANS") {
                if std::mem::replace(&mut nocptrans, true) {
                    return Err(SqlError::invalid_create(
                        " Redundant NOCPTRANS specification.",
                        pos,
                    ));
                }
            } else if self.eat_word("DEFAULT") {
                // A value the dictionary keeps, if it is a string; what
                // else comes is not read.
                match self.string() {
                    Some(value) => rules.default = Some(value),
                    None => self.skip_column_rest(),
                }
                constraint.get_or_insert(pos);
            } else if self.eat_word("CONSTRAINT") {
                if self.at_word("NOT") && self.at_word_at(1, "NULL") {
                    self.i += 2;
                    rules.not_null = true;
                } else {
                    let rule = if self.at_word("ERROR") && self.at_word_at(1, "MESSAGE") {
                        self.i += 2;
                        Some(&mut rules.message)
                    } else if self.eat_word("MINIMUM") {
                        Some(&mut rules.minimum)
                    } else if self.eat_word("MAXIMUM") {
                        Some(&mut rules.maximum)
                    } else {
                        None
                    };
                    let value = rule.map(|rule| (rule, self.string()));
                    match value {
                        Some((rule, Some(value))) => *rule = Some(value),
                        _ => self.skip_column_rest(),
                    }
                }
                constraint.get_or_insert(pos);
            } else {
                break;
            }
        }
        Ok(ColumnDef {
            name,
            name_pos,
            type_name,
            type_pos,
            params,
            compressed,
            nullable,
            nocptrans,
            rules,
        })
    }

    /// `ALTER TABLE`, after `TABLE`; `online` if `ONLINE` or
    /// `ONLINE_WITH_TRANSITION` came before it, `transition` for the latter.
    fn alter_table(&mut self, online: bool, transition: bool) -> Result<AlterTable, SqlError> {
        let table = self.own_target("ALTER TABLE")?;
        let table_pos = table.pos;
        let mut alter = AlterTable {
            table,
            actions: Vec::new(),
            constraint: None,
            online,
            transition,
        };
        // Whether `IGNORE` (true) or `RESPECT TRANSACTIONS` has been given.
        let mut transactions: Option<bool> = None;
        loop {
            let position = |parser: &mut Self| -> Result<Option<u64>, SqlError> {
                match parser.eat_word("POSITION") {
                    true => Ok(Some(parser.integer()?)),
                    false => Ok(None),
                }
            };
            let action = if self.eat_word("ADD") {
                self.eat_word("COLUMN");
                let column = self.column_def(&mut alter.constraint)?;
                AlterAction::Add(column, position(self)?)
            } else if self.eat_word("ALTER") {
                self.eat_word("COLUMN");
                let old_pos = self.pos();
                let old = self.name().ok_or_else(|| {
                    self.expected_here("<identifier> missing column name after \"ALTER\"")
                })?;
                if self.eat_word("DROP") {
                    let rule = self.column_rule()?;
                    AlterAction::DropRule(old, old_pos, rule)
                } else {
                    let column = self.column_def(&mut alter.constraint)?;
                    AlterAction::Alter(old, column, position(self)?)
                }
            } else if self.eat_word("DROP") {
                self.alter_drop()?
            } else if self.eat_word("MEMOBLOCKSIZE") {
                let pos = self.pos();
                match self.integer()? {
                    // The original engine reads a 0 as no action at all.
                    0 => {
                        return Err(SqlError::expected_explained(
                            "",
                            " Expecting either \"ADD\", \"ALTER\", or \"DROP\".",
                            pos,
                        ));
                    }
                    size => AlterAction::MemoBlockSize(size),
                }
            } else if self.at_word("IGNORE") || self.at_word("RESPECT") {
                let ignore = self.at_word("IGNORE");
                self.i += 1;
                if !self.at_word("TRANSACTIONS") {
                    let word = if ignore { "IGNORE" } else { "RESPECT" };
                    return Err(SqlError::expected(
                        &format!("Expected keyword TRANSACTIONS after {word}"),
                        self.grant_pos(),
                        "",
                    ));
                }
                let pos = self.pos();
                self.i += 1;
                if transactions.is_some_and(|before| before != ignore) {
                    return Err(SqlError::both_transaction_options(pos));
                }
                transactions = Some(ignore);
                AlterAction::Transactions(ignore)
            } else if alter.actions.is_empty() {
                return Err(SqlError::expected_explained(
                    "",
                    " Expecting either \"ADD\", \"ALTER\", or \"DROP\".",
                    // The original engine knows `RENAME` and places the error
                    // at it, at the table for any other word.
                    if self.at_word("RENAME") {
                        self.pos()
                    } else {
                        table_pos
                    },
                ));
            } else {
                break;
            };
            alter.actions.push(action);
        }
        Ok(alter)
    }

    /// What `ALTER COLUMN column DROP` takes away, after `DROP`.
    fn column_rule(&mut self) -> Result<ColumnRule, SqlError> {
        let Some(token) = self.peek().cloned() else {
            return Err(SqlError::expected(
                "<keyword> expecting constraint information after DROP.",
                self.grant_pos(),
                "",
            ));
        };
        let rule = match &token.tok {
            tok if tok.is("DEFAULT") => ColumnRule::Default,
            tok if tok.is("MINIMUM") => ColumnRule::Minimum,
            tok if tok.is("MAXIMUM") => ColumnRule::Maximum,
            tok if tok.is("ERROR") => {
                self.i += 1;
                if !self.at_word("MESSAGE") {
                    return Err(SqlError::expected("MESSAGE", self.grant_pos(), ""));
                }
                ColumnRule::ErrorMessage
            }
            tok if tok.is("NOT") => {
                self.i += 1;
                if !self.at_word("NULL") {
                    return Err(SqlError::expected("NULL", self.grant_pos(), ""));
                }
                ColumnRule::NotNull
            }
            tok => {
                return Err(SqlError::unexpected(
                    &format!(
                        "{} Expecting either \"DEFAULT\", \"MINIMUM\", \"MAXIMUM\", \"ERROR\", \
                         or \"NOT\".",
                        tok.text()
                    ),
                    token.pos,
                ));
            }
        };
        self.i += 1;
        Ok(rule)
    }

    /// What `ALTER TABLE … DROP` drops, after `DROP`: a column, or the
    /// primary key.
    fn alter_drop(&mut self) -> Result<AlterAction, SqlError> {
        let constraint = self.eat_word("CONSTRAINT");
        if constraint || self.at_word("PRIMARY") {
            if !self.eat_word("PRIMARY") {
                return Err(SqlError::expected("PRIMARY", self.grant_pos(), ""));
            }
            if !self.eat_word("KEY") {
                return Err(SqlError::expected("KEY", self.grant_pos(), ""));
            }
            return Ok(AlterAction::DropPrimaryKey);
        }
        if !self.eat_word("COLUMN")
            && matches!(self.peek_at(0), Some(Tok::Ident(word))
                if KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(word)))
        {
            return Err(SqlError::expected(
                "Expected column name or table constraint after \"DROP\"",
                self.pos(),
                "",
            ));
        }
        let name = self.name().ok_or_else(|| {
            self.expected_here("Missing column name or table constraint after \"DROP\"")
        })?;
        Ok(AlterAction::Drop(name))
    }

    /// `SET ::scope.variable = value`, after `SET`, which was at `set`.
    fn set_variable(&mut self, set: Pos) -> Result<SetVariable, SqlError> {
        let missing = "Expecting keyword TRANSACTION or system variable identifier after SET.";
        let (variable, pos) = match self.peek().cloned() {
            Some(Token {
                tok: Tok::Ident(name),
                pos,
                ..
            }) if name.starts_with("::") => (name, pos),
            Some(Token {
                tok: Tok::Ident(_),
                pos,
                ..
            }) => {
                // The original engine reads a qualified name whole, and
                // places the error at its last part.
                let at = match (self.peek_at(1), self.peek_at(2)) {
                    (Some(Tok::Punct(".")), Some(Tok::Ident(_))) => self.tokens[self.i + 2].pos,
                    _ => pos,
                };
                return Err(SqlError::expected(missing, at, ""));
            }
            Some(token) => {
                return Err(SqlError::expected(
                    &format!("<identifier>{missing}"),
                    token.pos,
                    "",
                ));
            }
            None => {
                return Err(SqlError::expected(
                    &format!("<identifier>{missing}"),
                    set,
                    "",
                ));
            }
        };
        let Some((scope, member)) = variable[2..].split_once('.').filter(|(_, m)| !m.is_empty())
        else {
            return Err(SqlError::expected(missing, pos, ""));
        };
        let member_pos = pos + 2 + scope.chars().count() + 1;
        let (scope, member) = (scope.to_ascii_lowercase(), member.to_owned());
        self.i += 1;
        // Errors at the end of the text are at its last character.
        let last = self.end;
        if !self.eat_punct("=") {
            let at = self.peek().map_or(last, |t| t.pos);
            return Err(SqlError::expected(
                "Missing '=' in the SET command.",
                at,
                "",
            ));
        }
        if self.peek().is_none() {
            return Err(SqlError::expected("identifier or expression", last, ""));
        }
        let value = self.value()?;
        Ok(SetVariable {
            scope,
            member,
            member_pos,
            value,
        })
    }

    /// `DROP TABLE name` or `DROP INDEX table.name`, after `DROP`.
    fn drop(&mut self) -> Result<Statement, SqlError> {
        if self.eat_word("TABLE") {
            let table = self.own_target("DROP TABLE")?;
            let from_database = self.eat_word("FROM");
            if from_database && !self.eat_word("DATABASE") {
                return Err(SqlError::expected("DATABASE", self.grant_pos(), ""));
            }
            let no_delete = from_database && self.eat_word("NO_DELETE");
            return Ok(Statement::DropTable(Box::new(DropTable {
                table,
                from_database,
                no_delete,
            })));
        }
        self.expect_word("INDEX")?;
        let pos = self.pos();
        let table = self.name().ok_or_else(|| {
            self.expected_here(
                "<identifier> missing table name You are missing the table name after the INDEX \
                 keyword. Advantage requires the table name in order to drop an index.",
            )
        })?;
        self.expect_punct_explained(
            ".",
            " You are missing the \".\" between the table and index names.",
        )?;
        let name_pos = self.pos();
        let name = self.name().ok_or_else(|| {
            self.expected_here(
                "<identifier> You are missing the name of the index after the \".\" seperator.",
            )
        })?;
        let id = self.tables;
        self.tables += 1;
        let table = TableRef {
            name: table,
            link: None,
            alias: None,
            pos,
            alias_pos: pos,
            id,
        };
        Ok(Statement::DropIndex(
            table,
            Named {
                name,
                pos: name_pos,
            },
        ))
    }

    /// `CACHE PREPARE ON | OFF | DEFAULT`, after `CACHE`.
    fn cache_prepare(&mut self) -> Result<Statement, SqlError> {
        if !self.eat_word("PREPARE") {
            return Err(SqlError::cache_without_prepare());
        }
        let mode = match self.peek().map(|t| &t.tok) {
            Some(t) if t.is("ON") => CachePrepare::On,
            Some(t) if t.is("OFF") => CachePrepare::Off,
            Some(t) if t.is("DEFAULT") => CachePrepare::Default,
            Some(Tok::Ident(_)) => {
                return Err(SqlError::expected(
                    "Expect ON, OFF or DEFAULT.",
                    self.pos(),
                    "",
                ));
            }
            _ => {
                return Err(SqlError::expected(
                    "Expecting ON, OFF or DEFAULT after PREPARE.",
                    self.pos(),
                    "",
                ));
            }
        };
        self.i += 1;
        Ok(Statement::CachePrepare(mode))
    }

    /// A command of the SQL debugger, after `DEBUG` at `pos`. Those that
    /// take arguments are not run yet; without them, they are the original
    /// engine's syntax errors.
    fn debug(&mut self, pos: Pos) -> Result<Statement, SqlError> {
        let Some(command) = self.next() else {
            return Err(SqlError::expected(
                "<keyword> You are missing the BEGIN, END, DATABASE, CONNECTION, RUN, WAIT, \
                 BREAK or REMOVE keyword after the DEBUG keyword.",
                pos,
                "",
            ));
        };
        let word = command.tok.text().to_ascii_uppercase();
        let debug = match word.as_str() {
            "BEGIN" => Debug::Begin,
            "END" => Debug::End,
            "WAIT" => Debug::Wait,
            "DATABASE" => Debug::Database,
            "CONNECTION" | "RUN" | "BREAK" | "REMOVE" => {
                if self.peek().is_some() && !self.at_punct(";") {
                    return Err(SqlError::debug_unsupported(&word, command.pos));
                }
                let missing = match word.as_str() {
                    "CONNECTION" => "<identifier> Missing connection name.".to_owned(),
                    "REMOVE" => {
                        " Expecting keywords BREAK POINT after the keyword REMOVE.".to_owned()
                    }
                    _ => format!("<identifier> Missing connection name after the keyword {word}."),
                };
                return Err(SqlError::expected(&missing, self.end, ""));
            }
            _ => {
                return Err(SqlError::unknown_debug_command(
                    &command.tok.text(),
                    command.pos,
                ));
            }
        };
        Ok(Statement::Debug(debug, command.pos))
    }

    /// `GRANT …` or `REVOKE …` after its word (probed,
    /// `fixtures/dictionary/grants.json`): `[GRANT OPTION FOR]` on a revoke, the
    /// permissions, `ON object` unless they are all database-wide, `TO`/`FROM`
    /// grantee, `WITH GRANT` on a grant. An error at the end of the text is at the
    /// last word.
    fn grant(&mut self, revoke: bool) -> Result<Grant, SqlError> {
        let mut grant_option = false;
        if revoke && self.at_word("GRANT") {
            let pos = self.grant_pos();
            if self.at_word_at(1, "OPTION") {
                self.i += 2;
                if !self.eat_word("FOR") {
                    return Err(SqlError::expected_explained(
                        "",
                        " You are missing the FOR keyword after GRANT OPTION.",
                        pos,
                    ));
                }
                grant_option = true;
            }
        }
        let mut permissions = Vec::new();
        loop {
            permissions.push(self.permission()?);
            if !self.eat_punct(",") {
                break;
            }
        }
        let database_wide = permissions.iter().any(|p| !p.needs_object());
        let on = self.grant_pos();
        let object = if self.eat_word("ON") {
            if database_wide {
                return Err(SqlError::not_supported());
            }
            // A missing object is placed at the `ON`.
            let object = self.grant_name().ok_or_else(|| {
                SqlError::expected_explained(
                    "<identifier>",
                    " You are missing the object identifier.  It needs to come after the ON \
                     keyword.",
                    on,
                )
            })?;
            Some(object)
        } else if !database_wide {
            return Err(SqlError::expected_explained(
                "",
                " The ON <object> clause is required for object permissions.",
                self.grant_pos(),
            ));
        } else {
            None
        };
        let to = if revoke { "FROM" } else { "TO" };
        if !self.eat_word(to) {
            return Err(SqlError::expected_explained(
                to,
                &format!(" The {to} keyword needs to come after the object identifier."),
                self.grant_pos(),
            ));
        }
        let grantee = self.grant_name().ok_or_else(|| {
            SqlError::expected_explained(
                "<identifier>",
                " You are missing the grantee identifier.  It needs to come after the TO keyword.",
                self.grant_pos(),
            )
        })?;
        if !revoke && self.at_word("WITH") {
            let pos = self.grant_pos();
            self.i += 1;
            if !self.eat_word("GRANT") {
                return Err(SqlError::expected_explained(
                    "",
                    " You are missing the GRANT keyword after WITH.",
                    pos,
                ));
            }
            grant_option = true;
        }
        Ok(Grant {
            revoke,
            grant_option,
            permissions,
            object,
            grantee,
        })
    }

    /// One permission of a `GRANT` or `REVOKE`.
    fn permission(&mut self) -> Result<Permission, SqlError> {
        const CREATES: [(&str, u16); 9] = [
            ("TABLE", 1),
            ("VIEW", 6),
            ("USER", 8),
            ("PROCEDURE", 10),
            ("LINK", 12),
            ("PUBLICATION", 15),
            ("SUBSCRIPTION", 17),
            ("FUNCTION", 18),
            ("PACKAGE", 19),
        ];
        // The kind of object after `CREATE` at `ahead`, and the words it takes.
        let creates = |p: &Parser, ahead: usize| {
            let (_, object_type) = CREATES.iter().find(|(w, _)| p.at_word_at(ahead, w))?;
            match object_type {
                8 if p.at_word_at(ahead + 1, "GROUP") => Some((9, 2)),
                &object_type => Some((object_type, 1)),
            }
        };
        let missing = |p: &Parser| {
            SqlError::expected_explained(
                "",
                " You are missing the permission identifier.",
                p.grant_pos(),
            )
        };
        let Some(Tok::Ident(word)) = self.peek().map(|t| &t.tok) else {
            return Err(missing(self));
        };
        let word = word.to_ascii_uppercase();
        let object = |right| Permission::Object {
            right,
            column: None,
        };
        let permission = match word.as_str() {
            "SELECT" | "INSERT" | "UPDATE" => {
                let right = match word.as_str() {
                    "SELECT" => 0x1,
                    "INSERT" => 0x10,
                    _ => 0x2,
                };
                self.i += 1;
                if !self.eat_punct("(") {
                    return Ok(object(right));
                }
                let column = self.grant_name().ok_or_else(|| {
                    SqlError::expected_explained(
                        "<identifier>",
                        " You are missing the column identifier.",
                        self.grant_pos(),
                    )
                })?;
                if !self.eat_punct(")") {
                    return Err(SqlError::expected_explained(
                        ")",
                        " You are missing the closing ')' after the column name.",
                        self.grant_pos(),
                    ));
                }
                return Ok(Permission::Object {
                    right,
                    column: Some(column),
                });
            }
            "DELETE" => object(0x20),
            "EXECUTE" => object(0x4),
            "ACCESS" => object(0x40),
            "DROP" => object(0x200),
            "ALL" => Permission::All,
            "ALTER" if self.at_word_at(1, "DATABASE") => {
                self.i += 1;
                Permission::Database(0x100)
            }
            "ALTER" => object(0x100),
            "INHERIT" if self.at_word_at(1, "DATABASE") => {
                self.i += 1;
                Permission::Database(0x8)
            }
            "INHERIT" if self.at_word_at(1, "CREATE") => match creates(self, 2) {
                Some((object_type, words)) => {
                    self.i += 1 + words;
                    Permission::Create {
                        object_type,
                        inherit: true,
                    }
                }
                None => object(0x8),
            },
            "INHERIT" => object(0x8),
            "CREATE" => {
                let (object_type, words) = creates(self, 1).ok_or_else(|| missing(self))?;
                self.i += words;
                Permission::Create {
                    object_type,
                    inherit: false,
                }
            }
            _ => return Err(missing(self)),
        };
        self.i += 1;
        Ok(permission)
    }

    /// A name in a `GRANT` or `REVOKE`: an identifier that is no reserved word,
    /// or a quoted one.
    fn grant_name(&mut self) -> Option<String> {
        match self.peek().map(|t| &t.tok) {
            Some(Tok::Ident(name)) if KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(name)) => None,
            _ => self.name(),
        }
    }

    /// Where a `GRANT` or `REVOKE` error is: the next token, or the last one at
    /// the end of the text.
    fn grant_pos(&self) -> Pos {
        match self.peek() {
            Some(_) => self.pos(),
            None => self.tokens.last().map_or(0, |t| t.pos),
        }
    }

    /// 2115 at [`Parser::grant_pos`]: `what` is expected, worded as the
    /// original engine words it.
    fn expected_here(&self, what: &str) -> SqlError {
        SqlError::expected(what, self.grant_pos(), "")
    }

    /// [`Parser::expect_punct`] for the punctuation the original engine
    /// explains, `why`, at [`Parser::grant_pos`].
    fn expect_punct_explained(&mut self, p: &str, why: &str) -> Result<(), SqlError> {
        if self.eat_punct(p) {
            return Ok(());
        }
        Err(SqlError::expected_explained(p, why, self.grant_pos()))
    }

    fn execute(&mut self) -> Result<Execute, SqlError> {
        let keyword = self.tokens[self.i - 1].pos;
        if !self.at_word("PROCEDURE") {
            return Err(SqlError::expected(
                " Expecting keyword PROCEDURE, IMMEDIATE, DPRINT or PRINT after the EXECUTE \
                 keyword.",
                self.pos(),
                "",
            ));
        }
        self.i += 1;
        let pos = self.pos();
        let name = self.name().ok_or_else(|| self.expected("<identifier>"))?;
        self.expect_punct("(")?;
        let mut args = Vec::new();
        if !self.eat_punct(")") {
            loop {
                args.push(self.argument()?);
                if self.eat_punct(")") {
                    break;
                }
                self.expect_punct(",")?;
            }
        }
        Ok(Execute {
            name,
            keyword,
            pos,
            args,
        })
    }

    /// A name: an identifier or a quoted one.
    fn name(&mut self) -> Option<String> {
        match self.peek().map(|t| &t.tok) {
            Some(Tok::Ident(name) | Tok::Quoted(name)) => {
                let name = name.clone();
                self.i += 1;
                Some(name)
            }
            _ => None,
        }
    }

    /// A `SELECT` after its word: its `UNION`s and `ORDER BY` included.
    fn select(&mut self) -> Result<Select, SqlError> {
        self.select_into(false)
    }

    /// [`Parser::select`], taking an `INTO` table if `into`: only the first
    /// `SELECT` of a statement does.
    fn select_into(&mut self, into: bool) -> Result<Select, SqlError> {
        let mut select = self.select_core(into)?;
        while self.eat_word("UNION") {
            let all = self.eat_word("ALL");
            if self.at_punct("(") {
                return Err(self.unmatched_parenthesis());
            }
            self.expect_word("SELECT")?;
            let part = self.select_core(false)?;
            select.unions.push((all, part));
        }
        select.order = self.order_by()?;
        Ok(select)
    }

    /// A statement that starts with `(`: the original takes a `SELECT` in
    /// parentheses only where a `UNION` follows (or is inside) it, and only
    /// when the statement starts so.
    fn grouped_select(&mut self) -> Result<Select, SqlError> {
        let mut select = self.union_operand()?;
        if !self.at_word("UNION") {
            return Err(self.after_group());
        }
        self.union_rest(&mut select)?;
        select.order = self.order_by()?;
        Ok(select)
    }

    /// The `UNION`s after `select`, each of a `SELECT` or one in parentheses.
    fn union_rest(&mut self, select: &mut Select) -> Result<(), SqlError> {
        while self.eat_word("UNION") {
            let all = self.eat_word("ALL");
            let mut part = self.union_operand()?;
            let rest = std::mem::take(&mut part.unions);
            select.unions.push((all, part));
            select.unions.extend(rest);
        }
        Ok(())
    }

    /// A `SELECT`, or `(…)` of them with a `UNION` between; one without is
    /// refused, at the end of its last token.
    fn union_operand(&mut self) -> Result<Select, SqlError> {
        if !self.eat_punct("(") {
            self.expect_word("SELECT")?;
            return self.select_core(false);
        }
        let mut select = self.union_operand()?;
        self.union_rest(&mut select)?;
        if !self.at_punct(")") {
            return Err(self.after_group());
        }
        if select.unions.is_empty() {
            return Err(SqlError::expected("UNION", self.tokens[self.i - 1].pos, ""));
        }
        self.i += 1;
        Ok(select)
    }

    /// What the original wants after a group that is not followed by a
    /// `UNION`; at the end of the text it is at the last token.
    fn after_group(&self) -> SqlError {
        let pos = self.tokens.get(self.i).map_or_else(
            || self.tokens.last().map_or(0, |t| t.pos),
            |token| token.pos,
        );
        SqlError::expected("UNION or ')'", pos, "")
    }

    /// A parenthesis after `UNION` in a statement that does not start with
    /// one, found at the first `)` or the end of the text.
    fn unmatched_parenthesis(&mut self) -> SqlError {
        while self.eat_punct("(") {}
        if self.eat_word("SELECT") && self.select_core(false).is_err() {
            return self.expected("identifier or expression");
        }
        let pos = self.tokens.get(self.i).map_or_else(
            || self.tokens.last().map_or(0, |t| t.pos),
            |token| token.pos,
        );
        SqlError::expected(
            "UNION or ) is expected. There is unmatched parenthesis.",
            pos,
            "",
        )
    }

    /// A `SELECT` up to its `HAVING`.
    fn select_core(&mut self, allow_into: bool) -> Result<Select, SqlError> {
        let outer = self.context;
        let mut force_static = self.static_escape(true);
        let top = self.top()?;
        force_static |= self.static_escape(false);
        let distinct = self.eat_word("DISTINCT");
        if !distinct {
            self.eat_word("ALL");
        }
        let mut items = Vec::new();
        loop {
            self.context = Context::Column(items.len() + 1);
            let pos = self.pos();
            if items.is_empty() && self.eat_punct("*") {
                items.push(Item::Star { table: None, pos });
                // `*` is the whole select list.
                break;
            }
            if matches!(self.peek_at(0), Some(Tok::Ident(_) | Tok::Quoted(_)))
                && self.peek_at(1) == Some(&Tok::Punct("."))
                && self.peek_at(2) == Some(&Tok::Punct("*"))
            {
                let table = self.name();
                self.i += 2;
                items.push(Item::Star { table, pos });
            } else {
                let expr = self.expr()?;
                if self.at_word("AS")
                    && let Some(Tok::Ident(word)) = self.peek_at(1)
                    && KEYWORDS.iter().any(|k| word.eq_ignore_ascii_case(k))
                {
                    let pos = self.tokens[self.i + 1].pos;
                    return Err(SqlError::expected_explained(
                        "<identifier>",
                        &format!(
                            " [Parsing for Alias (column {} in the SELECT clause)]",
                            items.len() + 1
                        ),
                        pos,
                    ));
                }
                let alias = self.alias()?;
                items.push(Item::Expr { expr, alias });
            }
            if !self.eat_punct(",") {
                break;
            }
        }
        self.context = Context::None;
        let into = match allow_into && self.eat_word("INTO") {
            true => {
                let pos = self.pos();
                let reserved = matches!(self.peek_at(0), Some(Tok::Ident(w))
                    if RESERVED.iter().any(|r| w.eq_ignore_ascii_case(r)));
                let name = (!reserved).then(|| self.name()).flatten();
                let name = name.ok_or_else(|| self.expected("<identifier>"))?;
                Some(Named { name, pos })
            }
            false => None,
        };
        if !self.at_word("FROM") {
            // A number or text after a column is no start of the next
            // (`fixtures/sql/gram_cast.json`).
            // After an alias, though, the original wants the `FROM`.
            let aliased = matches!(items.last(), Some(Item::Expr { alias: Some(_), .. }));
            if self.at_literal() && !aliased {
                return Err(SqlError::expected_explained(
                    "Unexpected token while parsing for Boolean expression.",
                    &Self::trailer_in(Context::Column(items.len())),
                    self.pos(),
                ));
            }
            return Err(SqlError::expected_explained(
                "FROM Missing the keyword FROM after the column definitions in the SELECT \
                 statement.",
                &Self::trailer_in(outer),
                // The original engine reads `!=` as two tokens and is at the
                // second.
                match self.peek() {
                    Some(Token {
                        tok: Tok::Punct("!="),
                        pos,
                        ..
                    }) => pos + 1,
                    _ => self.pos(),
                },
            ));
        }
        let from_pos = self.pos();
        self.i += 1;
        let from = self.from(from_pos)?;
        let filter = if self.eat_word("WHERE") {
            self.context = Context::Where;
            let filter = self.expr()?;
            self.context = Context::None;
            Some(filter)
        } else {
            None
        };
        let mut group = Vec::new();
        if self.eat_word("GROUP") {
            self.expect_word("BY")?;
            self.context = Context::GroupBy;
            loop {
                group.push(self.order_key()?);
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.context = Context::None;
        }
        let having = if self.eat_word("HAVING") {
            self.context = Context::Having;
            let having = self.expr()?;
            self.context = Context::None;
            Some(having)
        } else {
            None
        };
        Ok(Select {
            force_static,
            top,
            distinct,
            items,
            into,
            from,
            filter,
            group,
            having,
            unions: Vec::new(),
            order: Vec::new(),
        })
    }

    /// An `ORDER BY` or `GROUP BY` key: a select list ordinal or an
    /// expression.
    fn order_key(&mut self) -> Result<OrderKey, SqlError> {
        Ok(match self.peek().cloned() {
            Some(Token {
                tok: Tok::Number(n),
                pos,
                ..
            }) if n.bytes().all(|b| b.is_ascii_digit()) && !self.next_continues_expression() => {
                self.i += 1;
                OrderKey::Ordinal(n.parse().unwrap_or(u64::MAX), pos)
            }
            _ => OrderKey::Expr(self.expr()?),
        })
    }

    fn order_by(&mut self) -> Result<Vec<OrderItem>, SqlError> {
        let mut order = Vec::new();
        if self.eat_word("ORDER") {
            self.expect_word("BY")?;
            self.context = Context::OrderBy;
            loop {
                let key = self.order_key()?;
                let descending = if self.eat_word("DESC") {
                    true
                } else {
                    self.eat_word("ASC");
                    false
                };
                order.push(OrderItem { key, descending });
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.context = Context::None;
        }
        Ok(order)
    }

    /// What follows `FROM` (at `from_pos`): tables, joined with `,` or
    /// `JOIN`.
    fn from(&mut self, from_pos: Pos) -> Result<From, SqlError> {
        let left = self.table_item(from_pos)?;
        self.joins(left, from_pos)
    }

    /// The kind of join at the next token, which is taken, if there is one.
    fn join_kind(&mut self) -> Option<JoinKind> {
        if self.at_word("JOIN") {
            return Some(JoinKind::Inner);
        }
        if self.at_word("INNER") && self.peek_at(1).is_some_and(|t| t.is("JOIN")) {
            self.i += 1;
            return Some(JoinKind::Inner);
        }
        let kind = [
            ("LEFT", JoinKind::Left),
            ("RIGHT", JoinKind::Right),
            ("FULL", JoinKind::Full),
        ]
        .into_iter()
        .find_map(|(word, kind)| self.at_word(word).then_some(kind))?;
        self.i += 1;
        self.eat_word("OUTER");
        Some(kind)
    }

    /// The joins that follow the table `left`. A join without its `ON`
    /// takes the joins that follow its table for its own table, which
    /// then wants the `ON` (`fixtures/sql/gram_cross.json`).
    fn joins(&mut self, mut left: From, from_pos: Pos) -> Result<From, SqlError> {
        loop {
            if self.eat_punct(",") {
                let right = self.table_item(from_pos)?;
                left = From::Join {
                    kind: JoinKind::Inner,
                    left: Box::new(left),
                    right: Box::new(right),
                    on: None,
                };
                continue;
            }
            let Some(kind) = self.join_kind() else {
                break;
            };
            self.expect_word("JOIN")?;
            // A missing table is where the next word is, else at the last.
            if matches!(self.peek().map(|t| &t.tok), Some(Tok::Ident(name))
                if RESERVED.iter().any(|r| name.eq_ignore_ascii_case(r)))
            {
                return Err(SqlError::expected_explained(
                    "<identifier>",
                    &format!(" -- Missing table name.{}", self.table_names_problem()),
                    self.pos(),
                ));
            }
            let mut right = self.table_item(self.pos_expected())?;
            let save = self.i;
            if !self.at_word("ON") && self.join_kind().is_some() {
                self.i = save;
                right = self.joins(right, from_pos)?;
            }
            if !self.eat_word("ON") {
                return Err(SqlError::expected_explained(
                    "ON",
                    &self.table_names_problem(),
                    self.pos_expected(),
                ));
            }
            let on = self.expr()?;
            left = From::Join {
                kind,
                left: Box::new(left),
                right: Box::new(right),
                on: Some(on),
            };
        }
        Ok(left)
    }

    /// A table, a `SELECT` in parentheses, joins in parentheses or an ODBC
    /// outer join, `{OJ …}`.
    fn table_item(&mut self, from_pos: Pos) -> Result<From, SqlError> {
        if self.at_punct("{") && !self.in_outer_join {
            return self.outer_join_escape();
        }
        if self.at_punct("(") {
            let pos = self.pos();
            self.i += 1;
            if self.eat_word("SELECT") {
                let outer = std::mem::replace(&mut self.context, Context::From);
                let select = self.subselect()?;
                self.context = outer;
                let alias = self.alias()?;
                return Ok(From::Derived {
                    select: Box::new(select),
                    alias,
                    pos,
                });
            }
            if self.at_word("EXECUTE") {
                return self.procedure_item();
            }
            let inner = self.from(from_pos)?;
            self.expect_punct(")")?;
            return Ok(inner);
        }
        Ok(From::Table(self.table_ref(from_pos)?))
    }

    /// `{OJ table join table ON … }`, at its `{`: joins of its own, which
    /// must start with a join, take no alias after the `}` and hold no
    /// other `{OJ` (`fixtures/sql/gram_oj.json`). An inner join, a cross or
    /// a natural one takes it too, but each wants its `ON`.
    fn outer_join_escape(&mut self) -> Result<From, SqlError> {
        const IN_FROM: &str =
            " There was a problem parsing the table names after the FROM keyword.";
        let open = self.pos();
        self.i += 1;
        // The word that follows is `OJ` alone; one that only starts with it
        // is where the original puts the error.
        let at = match self.peek() {
            Some(token) if token.tok.is("OJ") => {
                self.i += 1;
                None
            }
            Some(Token {
                tok: Tok::Ident(word),
                pos,
                ..
            }) if word.len() > 2 && word[..2].eq_ignore_ascii_case("oj") => Some(*pos),
            _ => Some(open),
        };
        if let Some(at) = at {
            return Err(SqlError::expected_explained("oj", IN_FROM, at));
        }
        let parenthesized = self.at_punct("(") && !self.at_word_at(1, "SELECT");
        self.in_outer_join = true;
        let joins = self.outer_joins(parenthesized);
        self.in_outer_join = false;
        joins
    }

    /// The joins of an `{OJ`, after its `OJ`; `parenthesized` if they start
    /// with joins in parentheses.
    fn outer_joins(&mut self, parenthesized: bool) -> Result<From, SqlError> {
        const IN_FROM: &str =
            " There was a problem parsing the table names after the FROM keyword.";
        let mut left = match parenthesized {
            // Joins in parentheses are joins, and the first of them an inner
            // one too.
            true => {
                self.i += 1;
                let inner = self.from(self.pos())?;
                if !matches!(inner, From::Join { .. }) {
                    return Err(SqlError::expected(
                        "<LEFT [OUTER] | FULL [OUTER] | RIGHT [OUTER] | INNER> JOIN",
                        self.pos(),
                        IN_FROM,
                    ));
                }
                self.expect_punct(")")?;
                inner
            }
            false => self.table_item(self.pos())?,
        };
        // The first join is a must, and it names its kind.
        let mut first = !parenthesized;
        loop {
            let kind = if self.at_word("JOIN") {
                JoinKind::Inner
            } else if (self.at_word("INNER") || self.at_word("NATURAL"))
                && self.at_word_at(1, "JOIN")
            {
                self.i += 1;
                JoinKind::Inner
            } else if let Some(kind) = [
                ("LEFT", JoinKind::Left),
                ("RIGHT", JoinKind::Right),
                ("FULL", JoinKind::Full),
            ]
            .into_iter()
            .find_map(|(word, kind)| self.at_word(word).then_some(kind))
            {
                self.i += 1;
                self.eat_word("OUTER");
                kind
            } else if first {
                return Err(SqlError::expected(
                    "<LEFT [OUTER] | FULL [OUTER] | RIGHT [OUTER]> JOIN",
                    self.pos(),
                    IN_FROM,
                ));
            } else if self.eat_punct("}") {
                return Ok(left);
            } else {
                return Err(SqlError::expected_explained(
                    "} missing }",
                    IN_FROM,
                    self.grant_pos(),
                ));
            };
            first = false;
            self.expect_word("JOIN")?;
            let right = self.table_item(self.pos())?;
            if !self.eat_word("ON") {
                return Err(SqlError::expected_explained("ON", IN_FROM, self.pos()));
            }
            let on = self.expr()?;
            left = From::Join {
                kind,
                left: Box::new(left),
                right: Box::new(right),
                on: Some(on),
            };
        }
    }

    /// An `EXECUTE PROCEDURE` in parentheses after `FROM`, from its word, and
    /// the alias it needs.
    fn procedure_item(&mut self) -> Result<From, SqlError> {
        const IN_FROM: &str =
            " There was a problem parsing the table names after the FROM keyword.";
        self.i += 1;
        if !self.at_word("PROCEDURE") {
            return Err(SqlError::expected(
                " Expecting keyword PROCEDURE, IMMEDIATE, DPRINT or PRINT after the EXECUTE \
                 keyword.",
                self.pos(),
                IN_FROM,
            ));
        }
        let execute = self.execute()?;
        if !self.at_punct(")") {
            return Err(SqlError::expected_explained(")", IN_FROM, self.pos()));
        }
        let pos = self.pos();
        self.i += 1;
        let alias = self.alias()?;
        if alias.is_none() {
            return Err(SqlError::procedure_alias(pos));
        }
        let id = self.tables;
        self.tables += 1;
        Ok(From::Procedure {
            execute,
            id,
            alias,
            pos,
        })
    }

    /// A `SELECT` in parentheses, after its word, and its `)`.
    fn subselect(&mut self) -> Result<Select, SqlError> {
        let context = self.context;
        // A column's problem is told apart from the others'.
        let trailer = match context {
            Context::None | Context::Column(_) => self.outer_trailer.clone(),
            _ => self.trailer(),
        };
        let outer = std::mem::replace(&mut self.outer_trailer, trailer);
        let select = self.select();
        self.outer_trailer = outer;
        let select = select?;
        self.context = context;
        self.expect_punct(")")?;
        Ok(select)
    }

    /// Whether the token after the next continues an expression (an
    /// operator after an `ORDER BY` number).
    fn next_continues_expression(&self) -> bool {
        matches!(
            self.peek_at(1),
            Some(Tok::Punct("+" | "-" | "*" | "/" | "%" | "|" | "&" | "^"))
        )
    }

    /// A hint in braces after `SELECT`: any of the optimizer's hint words
    /// in any case, one group only; whether `static` is among them. Only the
    /// first place takes more than `static` (`fixtures/sql/claims.json`).
    fn static_escape(&mut self, first: bool) -> bool {
        const HINTS: [&str; 6] = [
            "static",
            "NOJOINORDEROPT",
            "NOMISCOPT",
            "NOSUBQUERYOPT",
            "NOPUSHDOWNSORTOPT",
            "NOEXECUTIONOPT",
        ];
        if !self.at_punct("{") {
            return false;
        }
        let mut words = 0;
        let mut stat = false;
        while let Some(Tok::Ident(word)) = self.peek_at(1 + words) {
            if !HINTS.iter().any(|h| word.eq_ignore_ascii_case(h)) {
                return false;
            }
            stat |= word.eq_ignore_ascii_case("static");
            words += 1;
        }
        if words == 0
            || self.peek_at(1 + words) != Some(&Tok::Punct("}"))
            || !(first || (stat && words == 1))
        {
            return false;
        }
        self.i += words + 2;
        stat
    }

    /// `TOP n`: a `TOP` not followed by a number is a column (`TOP -5` is
    /// `TOP` minus 5), as in the original engine.
    fn top(&mut self) -> Result<Option<Top>, SqlError> {
        let counted = matches!(
            self.peek_at(1),
            Some(Tok::Number(n)) if n.bytes().all(|b| b.is_ascii_digit())
        );
        // The original engine reads the number up to the point and takes the
        // rest for an expression that has no operator before it.
        if let Some(Token {
            tok: Tok::Number(n),
            pos,
            ..
        }) = self.tokens.get(self.i + 1)
            && self.at_word("TOP")
            && let Some(dot) = n.find('.')
            && dot > 0
            && n[..dot].bytes().all(|b| b.is_ascii_digit())
        {
            return Err(SqlError::expected_explained(
                "Unexpected token while parsing for Boolean expression.",
                &Self::trailer_in(Context::Column(1)),
                pos + dot + 1,
            ));
        }
        if !(counted && self.eat_word("TOP")) {
            return Ok(None);
        }
        let count = self.top_integer()?;
        // `PERCENT` takes no `START AT`: a `START` after it is a column, as
        // is a `PERCENT` after `START AT`.
        if self.eat_word("PERCENT") {
            return Ok(Some(Top {
                count,
                start: 1,
                percent: true,
            }));
        }
        let mut start = 1;
        if self.eat_word("START") {
            self.expect_word("AT")?;
            start = self.top_integer()?.max(1);
        }
        Ok(Some(Top {
            count,
            start,
            percent: false,
        }))
    }

    /// A number of `TOP`, which the original engine keeps the low 32 bits of
    /// (`TOP 4294967297` is `TOP 1`).
    fn top_integer(&mut self) -> Result<u64, SqlError> {
        match self.peek().cloned() {
            Some(Token {
                tok: Tok::Number(n),
                ..
            }) if n.bytes().all(|b| b.is_ascii_digit()) => {
                self.i += 1;
                let wrapped = n.bytes().fold(0u32, |v, b| {
                    v.wrapping_mul(10).wrapping_add(u32::from(b - b'0'))
                });
                Ok(u64::from(wrapped))
            }
            _ => Err(self.expected("integer")),
        }
    }

    fn integer(&mut self) -> Result<u64, SqlError> {
        match self.peek().cloned() {
            Some(Token {
                tok: Tok::Number(n),
                ..
            }) if n.bytes().all(|b| b.is_ascii_digit()) => {
                self.i += 1;
                Ok(n.parse().unwrap_or(u64::MAX))
            }
            _ => Err(self.expected("integer")),
        }
    }

    /// Whether the next token starts a number, a text or a binary literal
    /// (`X'2A'`, whose quote follows at once).
    fn at_literal(&self) -> bool {
        match self.peek() {
            Some(Token {
                tok: Tok::Number(_) | Tok::Str(_),
                ..
            }) => true,
            Some(
                token @ Token {
                    tok: Tok::Ident(word),
                    ..
                },
            ) if word.eq_ignore_ascii_case("X") => self
                .tokens
                .get(self.i + 1)
                .is_some_and(|next| matches!(next.tok, Tok::Str(_)) && next.pos == token.end),
            _ => false,
        }
    }

    /// A column alias: `AS name`, or a name that is not a keyword.
    fn alias(&mut self) -> Result<Option<String>, SqlError> {
        if self.eat_word("AS") {
            return self
                .name()
                .map(Some)
                .ok_or_else(|| self.expected("<identifier>"));
        }
        Ok(match self.peek().map(|t| &t.tok) {
            Some(Tok::Ident(name))
                if !RESERVED.iter().any(|r| name.eq_ignore_ascii_case(r)) && !self.at_literal() =>
            {
                let name = name.clone();
                self.i += 1;
                Some(name)
            }
            Some(Tok::Quoted(name)) => {
                let name = name.clone();
                self.i += 1;
                Some(name)
            }
            _ => None,
        })
    }

    /// The table after `FROM`, which is at `from_pos`.
    fn table_ref(&mut self, from_pos: Pos) -> Result<TableRef, SqlError> {
        let pos = self.pos();
        // A procedure takes parentheses; the word is no table's name.
        let procedure = self.at_word("EXECUTE");
        // After an `UPDATE`'s `FROM`, a keyword is no table's name either.
        let keyword = self.context == Context::UpdateFrom
            && matches!(self.peek().map(|t| &t.tok), Some(Tok::Ident(name))
                if RESERVED.iter().any(|r| name.eq_ignore_ascii_case(r)));
        let name = (!procedure && !keyword).then(|| self.name()).flatten();
        let name = name.ok_or_else(|| {
            SqlError::expected_explained(
                "<identifier>",
                &format!(" -- Missing table name.{}", self.table_names_problem()),
                if procedure || keyword { pos } else { from_pos },
            )
        })?;
        let (link, name) = self.qualified(name);
        // A name of more parts is no table the original engine knows.
        if link.is_some() && self.at_punct(".") {
            return Err(SqlError::unexpected_token(".", self.pos()));
        }
        let alias_pos = self.pos();
        let alias = self.alias()?;
        let id = self.tables;
        self.tables += 1;
        let link = link.map(|(link, _)| link);
        Ok(TableRef {
            name,
            link,
            alias,
            pos,
            alias_pos,
            id,
        })
    }

    /// The table named `name` and what follows it: `system.<table>` (a
    /// name of its own), else perhaps `<link>.<table>`, a table of the
    /// dictionary the link `name` reaches, with where the original engine
    /// places errors about such a name (just after the dot).
    fn qualified(&mut self, mut name: String) -> (Option<(String, Pos)>, String) {
        let part = |parser: &Self| {
            parser.at_punct(".")
                && matches!(parser.peek_at(1), Some(Tok::Ident(_) | Tok::Quoted(_)))
        };
        if name.eq_ignore_ascii_case("system") {
            while part(self) {
                self.i += 1;
                name.push('.');
                name.push_str(&self.name().unwrap());
            }
            return (None, name);
        }
        if !part(self) {
            return (None, name);
        }
        let at = self.pos();
        self.i += 1;
        let table = self.name().unwrap();
        (Some((name, at)), table)
    }

    /// The table an `INSERT`, `UPDATE` or `DELETE` changes: a name, without an
    /// alias; a token that is none is explained by `missing`.
    fn dml_target(&mut self, missing: &str) -> Result<TableRef, SqlError> {
        Ok(self.qualified_target(Some(missing))?.0)
    }

    /// [`Parser::target`], and where the original engine places errors
    /// about its link if it names one.
    fn qualified_target(
        &mut self,
        missing: Option<&str>,
    ) -> Result<(TableRef, Option<Pos>), SqlError> {
        let pos = self.pos();
        let name = self.name().ok_or_else(|| match missing {
            Some(why) => SqlError::expected_explained("<identifier>", why, self.grant_pos()),
            None => self.expected("<identifier>"),
        })?;
        let (link, name) = self.qualified(name);
        let id = self.tables;
        self.tables += 1;
        let at = link.as_ref().map(|&(_, at)| at);
        let link = link.map(|(link, _)| link);
        Ok((
            TableRef {
                name,
                link,
                alias: None,
                pos,
                alias_pos: pos,
                id,
            },
            at,
        ))
    }

    /// The table `DROP TABLE` or `ALTER TABLE` (`statement`) changes, which
    /// may not be a link's.
    fn own_target(&mut self, statement: &str) -> Result<TableRef, SqlError> {
        match self.qualified_target(Some(" -- Missing table name."))? {
            (table, Some(at)) => {
                let name = format!("{}.{}", table.link.unwrap_or_default(), table.name);
                Err(SqlError::across_link(&name, statement, at))
            }
            (table, None) => Ok(table),
        }
    }

    /// The table `CREATE TABLE` or `CREATE INDEX` names before its `(`, which
    /// may not be a link's: the original engine wants the `(` there,
    /// explained by `missing`, and words a missing name as `name`.
    fn created_target(&mut self, name: &str, missing: &str) -> Result<TableRef, SqlError> {
        match self.qualified_target(Some(name))? {
            (_, Some(at)) => Err(SqlError::expected_explained("(", missing, at)),
            (table, None) => Ok(table),
        }
    }

    /// `INSERT`, after its word.
    fn insert(&mut self) -> Result<Insert, SqlError> {
        if !self.eat_word("INTO") {
            return Err(SqlError::expected_explained(
                "INTO",
                " You are missing the INTO keyword after the INSERT keyword.",
                self.pos(),
            ));
        }
        let table = self.dml_target(
            " -- Missing table name. You are missing the table name after the INTO keyword in \
             INSERT INTO.",
        )?;
        let (columns, source) = self.insert_values()?;
        Ok(Insert {
            table,
            columns,
            source,
        })
    }

    /// What follows the table of an `INSERT` or of `MERGE`'s `INSERT`: the
    /// columns given values, and the values or the `SELECT` (`DEFAULT
    /// VALUES` gives no column a value).
    fn insert_values(&mut self) -> Result<(Option<Vec<Named>>, InsertSource), SqlError> {
        if self.at_word("DEFAULT") && self.at_word_at(1, "VALUES") {
            self.i += 2;
            return Ok((Some(Vec::new()), InsertSource::Values(vec![Vec::new()])));
        }
        let columns = match self.eat_punct("(") {
            true => {
                let mut columns = Vec::new();
                loop {
                    let pos = self.pos();
                    let name = self.name().ok_or_else(|| self.expected("<identifier>"))?;
                    columns.push(Named { name, pos });
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                if !self.eat_punct(")") {
                    return Err(SqlError::expected_explained(
                        ")",
                        " You are missing the closing \")\" after the column identifiers in \
                         INSERT INTO.",
                        self.pos_expected(),
                    ));
                }
                Some(columns)
            }
            false => None,
        };
        if self.eat_word("SELECT") {
            let select = self.select()?;
            return Ok((columns, InsertSource::Select(Box::new(select))));
        }
        if !self.eat_word("VALUES") {
            return Err(SqlError::expected_explained(
                "VALUES",
                " You are missing the keyword VALUES after the column identifiers in INSERT \
                 INTO.",
                self.pos_expected(),
            ));
        }
        let mut rows = Vec::new();
        loop {
            if !self.eat_punct("(") {
                return Err(SqlError::expected_explained(
                    "(",
                    " You are missing the opening \"(\" after the keyword VALUES in INSERT \
                     INTO.",
                    self.pos_expected(),
                ));
            }
            let mut row = Vec::new();
            self.context = Context::InsertValues;
            loop {
                row.push(self.insert_value()?);
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.context = Context::None;
            if !self.eat_punct(")") {
                return Err(match self.at_literal() {
                    true => SqlError::expected_explained(
                        "Unexpected token while parsing for Boolean expression.",
                        INSERT_VALUES_PROBLEM,
                        self.pos(),
                    ),
                    false => SqlError::expected_explained(
                        ")",
                        " You are missing the closing \")\" after the keyword VALUES in INSERT \
                         INTO.",
                        self.pos_expected(),
                    ),
                });
            }
            rows.push(row);
            if !self.eat_punct(",") {
                break;
            }
        }
        Ok((columns, InsertSource::Values(rows)))
    }

    /// A value of `INSERT`'s list: `DEFAULT` leaves the column as a new
    /// record has it.
    fn insert_value(&mut self) -> Result<Expr, SqlError> {
        if self.at_word("DEFAULT") && matches!(self.peek_at(1), Some(Tok::Punct(")" | ","))) {
            let pos = self.pos();
            self.i += 1;
            return Ok(Expr {
                kind: ExprKind::Default,
                pos,
            });
        }
        self.value()
    }

    /// `MERGE`, after its word.
    fn merge(&mut self) -> Result<Merge, SqlError> {
        self.eat_word("INTO");
        // `AS` takes no keyword of the statement for an alias.
        if self.at_word_at(1, "AS") && (self.at_word_at(2, "USING") || self.at_word_at(2, "ON")) {
            self.i += 2;
            return Err(self.expected("<identifier>"));
        }
        let from_pos = self.pos();
        let table = self.table_ref(from_pos)?;
        let source = match self.eat_word("USING") {
            true => {
                let at = self.pos();
                let source = self.table_item(at)?;
                if matches!(source, From::Derived { alias: None, .. }) {
                    return Err(SqlError::missing_alias(self.tokens[self.i - 1].pos));
                }
                Some(source)
            }
            false => None,
        };
        self.expect_word("ON")?;
        let on = self.expr()?;
        let (mut update, mut insert) = (None, None);
        self.expect_word("WHEN")?;
        let mut first = true;
        loop {
            // The second clause is the other one.
            let not = match first {
                true => self.eat_word("NOT"),
                false if update.is_some() => {
                    self.expect_word("NOT")?;
                    true
                }
                false => false,
            };
            self.expect_word("MATCHED")?;
            self.expect_word("THEN")?;
            match not {
                true => insert = Some(self.merge_insert()?),
                false => update = Some(self.merge_update()?),
            }
            if !(first && self.eat_word("WHEN")) {
                break;
            }
            first = false;
        }
        Ok(Merge {
            table,
            source,
            on,
            update,
            insert,
        })
    }

    /// `UPDATE [WITH DELETE | WITH RECALL] SET …` of a `MERGE`.
    fn merge_update(&mut self) -> Result<MergeUpdate, SqlError> {
        self.expect_word("UPDATE")?;
        let mark = self.mark()?;
        let sets = self.set_clause()?;
        Ok(MergeUpdate { mark, sets })
    }

    /// `[WITH DELETE | WITH RECALL]` after `UPDATE`'s table.
    fn mark(&mut self) -> Result<Mark, SqlError> {
        if !self.eat_word("WITH") {
            return Ok(Mark::Keep);
        }
        if self.eat_word("DELETE") {
            return Ok(Mark::Delete);
        }
        if self.eat_word("RECALL") {
            return Ok(Mark::Recall);
        }
        Err(SqlError::expected_explained(
            "DELETE",
            " You are missing the keyword RECALL or DELETE after the keyword WITH in the UPDATE \
             statement.",
            self.pos(),
        ))
    }

    /// `SET column = value, …` of an `UPDATE` or a `MERGE`'s.
    fn set_clause(&mut self) -> Result<Vec<MergeSet>, SqlError> {
        if !self.eat_word("SET") {
            return Err(SqlError::expected_explained(
                "SET",
                " You are missing the keyword SET after the table name in the UPDATE statement.",
                self.pos(),
            ));
        }
        let mut sets = Vec::new();
        let missing = |this: &Self| {
            SqlError::expected_explained("<identifier>", SET_LIST_PROBLEM, this.pos_expected())
        };
        loop {
            let pos = self.pos();
            let name = self.name().ok_or_else(|| missing(self))?;
            let (qualifier, name) = match self.at_punct(".") {
                true => {
                    self.i += 1;
                    let column = self.name().ok_or_else(|| missing(self))?;
                    (Some(name), column)
                }
                false => (None, name),
            };
            if !self.eat_punct("=") {
                return Err(SqlError::expected_explained(
                    "=",
                    " You are missing '=' in the UPDATE statement after a column name. \
                     There were problems parsing the SET list in the UPDATE statement.",
                    self.pos_expected(),
                ));
            }
            self.context = Context::SetList;
            // A keyword of the statement is no value.
            if self.at_word("WHERE") || self.at_word("FROM") {
                self.i += 1;
                return Err(self.expected("identifier or expression"));
            }
            let value = self.value()?;
            self.context = Context::None;
            sets.push(MergeSet {
                qualifier,
                column: Named { name, pos },
                value,
            });
            if !self.eat_punct(",") {
                break;
            }
        }
        Ok(sets)
    }

    /// `INSERT [WITH DELETE] …` of a `MERGE`.
    fn merge_insert(&mut self) -> Result<MergeInsert, SqlError> {
        self.expect_word("INSERT")?;
        let deleted = self.eat_word("WITH");
        if deleted {
            self.expect_word("DELETE")?;
        }
        let (columns, source) = self.insert_values()?;
        Ok(MergeInsert {
            deleted,
            columns,
            source,
        })
    }

    /// `UPDATE`, after its word.
    fn update(&mut self) -> Result<Update, SqlError> {
        self.static_escape(true);
        let table = self.dml_target(
            " -- Missing table name. You are missing the table name after the keyword UPDATE.",
        )?;
        let mark = self.mark()?;
        let sets = self.set_clause()?;
        let from = if self.at_word("FROM") {
            let from_pos = self.pos();
            self.i += 1;
            self.context = Context::UpdateFrom;
            let from = self.from(from_pos)?;
            self.context = Context::None;
            Some(from)
        } else {
            None
        };
        let filter = if self.eat_word("WHERE") {
            self.context = Context::UpdateWhere;
            let filter = self.expr()?;
            self.context = Context::None;
            Some(filter)
        } else {
            None
        };
        let named = table.name.clone();
        let table = match &from {
            Some(from) => update_target(table, from)?,
            None => table,
        };
        Ok(Update {
            table,
            named,
            mark,
            sets,
            from,
            filter,
        })
    }

    /// `DELETE`, after its word.
    fn delete(&mut self) -> Result<Delete, SqlError> {
        self.static_escape(true);
        if !self.eat_word("FROM") {
            return Err(SqlError::expected_explained(
                "FROM",
                " You are missing the keyword FROM after the keyword DELETE",
                self.pos(),
            ));
        }
        let table = self.dml_target(" -- Missing table name.")?;
        let filter = if self.eat_word("WHERE") {
            self.context = Context::DeleteWhere;
            let filter = self.expr()?;
            self.context = Context::None;
            Some(filter)
        } else {
            None
        };
        Ok(Delete { table, filter })
    }

    fn expr(&mut self) -> Result<Expr, SqlError> {
        let mut left = self.and()?;
        while self.at_word("OR") {
            self.i += 1;
            let right = self.and()?;
            left = binary(BinOp::Or, left, right);
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, SqlError> {
        let mut left = self.not()?;
        while self.at_word("AND") {
            self.i += 1;
            let right = self.not()?;
            left = binary(BinOp::And, left, right);
        }
        Ok(left)
    }

    fn not(&mut self) -> Result<Expr, SqlError> {
        let pos = self.pos();
        if self.eat_word("NOT") {
            let expr = self.not()?;
            return Ok(Expr {
                kind: ExprKind::Not(Box::new(expr)),
                pos,
            });
        }
        self.predicate()
    }

    fn predicate(&mut self) -> Result<Expr, SqlError> {
        let left = self.bitwise()?;
        let pos = left.pos;
        let op = match self.peek().map(|t| &t.tok) {
            Some(Tok::Punct("=")) => Some(BinOp::Eq),
            Some(Tok::Punct("<>")) => Some(BinOp::Ne),
            Some(Tok::Punct("<")) => Some(BinOp::Lt),
            Some(Tok::Punct("<=")) => Some(BinOp::Le),
            Some(Tok::Punct(">")) => Some(BinOp::Gt),
            Some(Tok::Punct(">=")) => Some(BinOp::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.i += 1;
            for (word, all) in [("ALL", true), ("ANY", false)] {
                if self.at_word(word)
                    && self.peek_at(1) == Some(&Tok::Punct("("))
                    && self.peek_at(2).is_some_and(|t| t.is("SELECT"))
                {
                    self.i += 3;
                    let select = self.subselect()?;
                    return Ok(Expr {
                        kind: ExprKind::Quantified {
                            op,
                            expr: Box::new(left),
                            select: Box::new(select),
                            all,
                        },
                        pos,
                    });
                }
            }
            let right = self.bitwise()?;
            return Ok(binary(op, left, right));
        }
        if self.eat_word("IS") {
            let negated = self.eat_word("NOT");
            self.expect_word_explained("NULL")?;
            return Ok(Expr {
                kind: ExprKind::IsNull {
                    expr: Box::new(left),
                    negated,
                },
                pos,
            });
        }
        let negated = self.at_word("NOT")
            && self
                .peek_at(1)
                .is_some_and(|t| t.is("BETWEEN") || t.is("IN") || t.is("LIKE"));
        if negated {
            self.i += 1;
        } else if self.context == Context::Where
            && self.at_word("NOT")
            && let Some(next) = self.tokens.get(self.i + 1)
        {
            // The place is the end of what follows the `NOT`.
            return Err(SqlError::expected(
                "Expecting IN, LIKE or BETWEEN after the keyword NOT. There was a problem \
                 parsing the WHERE clause.",
                next.end - 1,
                "",
            ));
        }
        if self.eat_word("BETWEEN") {
            let low = self.bitwise()?;
            self.expect_word_explained("AND")?;
            let high = self.bitwise()?;
            return Ok(Expr {
                kind: ExprKind::Between {
                    expr: Box::new(left),
                    low: Box::new(low),
                    high: Box::new(high),
                    negated,
                },
                pos,
            });
        }
        if self.eat_word("IN") {
            self.expect_punct("(")?;
            if self.eat_word("SELECT") {
                let select = self.subselect()?;
                return Ok(Expr {
                    kind: ExprKind::InSelect {
                        expr: Box::new(left),
                        select: Box::new(select),
                        negated,
                    },
                    pos,
                });
            }
            let mut list = Vec::new();
            loop {
                list.push(self.argument()?);
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct(")")?;
            return Ok(Expr {
                kind: ExprKind::In {
                    expr: Box::new(left),
                    list,
                    negated,
                },
                pos,
            });
        }
        if self.eat_word("LIKE") {
            let pattern = self.bitwise()?;
            let escape = if self.eat_word("ESCAPE") {
                // A text constant must be one ASCII character other than a
                // wildcard; the error is placed after it.
                if let Some(Token {
                    tok: Tok::Str(text),
                    end,
                    ..
                }) = self.peek()
                    && (text.len() != 1 || !text.is_ascii() || text == "%" || text == "_")
                {
                    return Err(SqlError::expected(
                        "ESCAPE clause must be a single ASCII character that is not '%' or '_'. \
                         There was a problem parsing the WHERE clause.",
                        *end - 1,
                        "",
                    ));
                }
                Some(Box::new(self.bitwise()?))
            } else {
                None
            };
            return Ok(Expr {
                kind: ExprKind::Like {
                    expr: Box::new(left),
                    pattern: Box::new(pattern),
                    escape,
                    negated,
                },
                pos,
            });
        }
        Ok(left)
    }

    fn bitwise(&mut self) -> Result<Expr, SqlError> {
        if self.in_call == 0 {
            // `ORDER BY` and `GROUP BY` place it at the failing token.
            self.anchor = match self.context {
                Context::OrderBy | Context::GroupBy => None,
                _ => self.i.checked_sub(1).map(|i| self.tokens[i].pos),
            };
        }
        let mut left = self.additive()?;
        loop {
            let op = match self.peek().map(|t| &t.tok) {
                Some(Tok::Punct("|")) => BinOp::BitOr,
                Some(Tok::Punct("&")) => BinOp::BitAnd,
                Some(Tok::Punct("^")) => BinOp::BitXor,
                Some(Tok::Punct("<<")) => BinOp::Shl,
                Some(Tok::Punct(">>")) => BinOp::Shr,
                _ => return Ok(left),
            };
            self.i += 1;
            let right = self.additive()?;
            left = binary(op, left, right);
        }
    }

    fn additive(&mut self) -> Result<Expr, SqlError> {
        let mut left = self.multiplicative()?;
        loop {
            let op = match self.peek().map(|t| &t.tok) {
                Some(Tok::Punct("+")) => BinOp::Add,
                Some(Tok::Punct("-")) => BinOp::Sub,
                _ => return Ok(left),
            };
            let at = self.pos();
            self.i += 1;
            let right = self.multiplicative()?;
            self.sum_pos = at;
            left = binary(op, left, right);
        }
    }

    fn multiplicative(&mut self) -> Result<Expr, SqlError> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek().map(|t| &t.tok) {
                Some(Tok::Punct("*")) => BinOp::Mul,
                Some(Tok::Punct("/")) => BinOp::Div,
                Some(Tok::Punct("%")) => BinOp::Mod,
                _ => return Ok(left),
            };
            self.i += 1;
            let right = self.unary()?;
            left = binary(op, left, right);
        }
    }

    fn unary(&mut self) -> Result<Expr, SqlError> {
        let pos = self.pos();
        if self.eat_punct("-") {
            // Unary minus takes no operand that starts with a minus.
            if self.at_punct("-") {
                return Err(self.expected("identifier or expression"));
            }
            let expr = self.unary()?;
            // A negative number is a literal of its own.
            if let ExprKind::Number(n) = &expr.kind {
                return Ok(Expr {
                    kind: ExprKind::Number(format!("-{n}")),
                    pos,
                });
            }
            return Ok(Expr {
                kind: ExprKind::Neg(Box::new(expr)),
                pos,
            });
        }
        if self.eat_punct("+") {
            let expr = self.unary()?;
            return Ok(Expr {
                kind: ExprKind::Plus(Box::new(expr)),
                pos,
            });
        }
        if self.eat_punct("~") {
            let expr = self.unary()?;
            return Ok(Expr {
                kind: ExprKind::BitNot(Box::new(expr)),
                pos,
            });
        }
        let expr = self.primary()?;
        Ok(expr)
    }

    /// A function or procedure argument, which may be `NULL`.
    /// A value an `INSERT` or `UPDATE` writes: NULL as well.
    fn value(&mut self) -> Result<Expr, SqlError> {
        if self.at_word("NULL")
            && (matches!(self.peek_at(1), None | Some(Tok::Punct(")" | "," | ";")))
                || self
                    .peek_at(1)
                    .is_some_and(|t| t.is("WHERE") || t.is("WHEN") || t.is("FROM")))
        {
            let pos = self.pos();
            self.i += 1;
            return Ok(Expr {
                kind: ExprKind::Null,
                pos,
            });
        }
        self.expr()
    }

    fn argument(&mut self) -> Result<Expr, SqlError> {
        self.argument_before(&["AS"])
    }

    /// A result of `CASE`, which a `WHEN`, `ELSE` or `END` ends.
    fn case_result(&mut self) -> Result<Expr, SqlError> {
        self.argument_before(&["AS", "WHEN", "ELSE", "END"])
    }

    fn argument_before(&mut self, words: &[&str]) -> Result<Expr, SqlError> {
        if self.at_word("NULL")
            && (matches!(self.peek_at(1), None | Some(Tok::Punct(")" | ",")))
                || self
                    .peek_at(1)
                    .is_some_and(|t| words.iter().any(|w| t.is(w))))
        {
            let pos = self.pos();
            self.i += 1;
            return Ok(Expr {
                kind: ExprKind::Null,
                pos,
            });
        }
        self.expr()
    }

    fn primary(&mut self) -> Result<Expr, SqlError> {
        let Some(token) = self.peek().cloned() else {
            return Err(self.expected("identifier or expression"));
        };
        let pos = token.pos;
        // `FETCH cursor` is a condition of its own, not an expression.
        if self.at_word("FETCH") && matches!(self.peek_at(1), Some(Tok::Ident(_))) {
            let at = self.tokens.get(self.i + 1).map_or(self.end, |t| t.pos);
            return Err(SqlError::expected(
                "identifier or expression",
                at,
                &self.trailer(),
            ));
        }
        // `FROM` and `THEN` end the expressions before them; the original
        // engine takes one for the operand and finds the next token wrong.
        if self.at_word("FROM") || self.at_word("THEN") {
            self.i += 1;
            return Err(self.expected("identifier or expression"));
        }
        let kind = match token.tok {
            Tok::Number(n) => {
                if n.matches('.').count() > 1 {
                    // The original reads the number up to its second point
                    // and places the error at the number itself.
                    let end = n.match_indices('.').nth(1).map_or(n.len(), |(at, _)| at);
                    return Err(SqlError::malformed_number_at_start(
                        &n[..end],
                        &self.trailer(),
                    ));
                }
                self.i += 1;
                ExprKind::Number(n)
            }
            Tok::Ident(ref word) if word.eq_ignore_ascii_case(".T.") => {
                self.i += 1;
                ExprKind::Bool(true)
            }
            Tok::Ident(ref word) if word.eq_ignore_ascii_case(".F.") => {
                self.i += 1;
                ExprKind::Bool(false)
            }
            Tok::Str(s) => {
                self.i += 1;
                ExprKind::Str(s)
            }
            Tok::Param(name) => {
                self.i += 1;
                // A named parameter that appears again is a parameter of its
                // own, as in the original engine
                // (`fixtures/sql/params5.json`).
                self.params.push(name);
                let index = self.params.len() - 1;
                ExprKind::Param(index)
            }
            Tok::Punct("(") => {
                self.i += 1;
                if self.eat_word("SELECT") {
                    let context = self.context;
                    let first = self.pos();
                    let select = self.subselect();
                    // In a column, the original engine says of any syntax
                    // error in a subselect that its first word is no
                    // expression (`fixtures/sql/gram_cross.json`).
                    let select = match (select, context) {
                        (Err(e), Context::Column(_)) if e.native == 2115 => {
                            return Err(SqlError::expected(
                                "identifier or expression",
                                first,
                                &Self::trailer_in(context),
                            ));
                        }
                        (select, _) => select?,
                    };
                    return Ok(Expr {
                        kind: ExprKind::Subquery(Box::new(select)),
                        pos,
                    });
                }
                let expr = self.expr()?;
                self.expect_punct(")")?;
                // A parenthesised condition that is none is pointed at inside
                // its parenthesis, at the operator of a sum.
                let pos = match expr.kind {
                    ExprKind::Binary(BinOp::Add | BinOp::Sub, ..) => self.sum_pos,
                    _ => expr.pos,
                };
                return Ok(Expr {
                    kind: expr.kind,
                    pos,
                });
            }
            Tok::Punct("{") => return self.escape(),
            Tok::Ident(ref word) if word.eq_ignore_ascii_case("NULL") => {
                // NULL is a value only where a function takes it; elsewhere
                // the original engine expects more after it.
                self.i += 1;
                return Err(self.expected("identifier or expression"));
            }
            // A `SELECT` where no subquery can start (`= SOME (SELECT …)`), or a
            // keyword of the statement, is read as a word that wants more
            // after it.
            Tok::Ident(ref word)
                if ["SELECT", "WHERE", "WHEN", "FROM"]
                    .iter()
                    .any(|w| word.eq_ignore_ascii_case(w)) =>
            {
                self.i += 1;
                return Err(self.expected("identifier or expression"));
            }
            Tok::Ident(ref word) if word.eq_ignore_ascii_case("TRUE") => {
                self.i += 1;
                ExprKind::Bool(true)
            }
            Tok::Ident(ref word) if word.eq_ignore_ascii_case("FALSE") => {
                self.i += 1;
                ExprKind::Bool(false)
            }
            Tok::Ident(ref word) if word.eq_ignore_ascii_case("CASE") => {
                self.i += 1;
                return self.case(pos);
            }
            // A system variable's scope with no variable after it.
            Tok::Ident(ref name) if name.starts_with("::") && name.ends_with('.') => {
                return Err(self.expected("identifier or expression"));
            }
            // `DATE'…'`, `TIME'…'` and `TIMESTAMP'…'`: the quote must follow
            // at once, a space after the word is no literal.
            Tok::Ident(ref word)
                if ["DATE", "TIME", "TIMESTAMP", "DATETIME"]
                    .iter()
                    .any(|w| word.eq_ignore_ascii_case(w))
                    && matches!(self.peek_at(1), Some(Tok::Str(_))) =>
            {
                let literal = match word.to_ascii_uppercase().as_str() {
                    "DATE" => TypedLiteral::Date,
                    "TIME" => TypedLiteral::Time,
                    _ => TypedLiteral::TimeStamp,
                };
                let string = self.tokens[self.i + 1].clone();
                if string.pos != token.end {
                    return Err(self.stray_text(&string));
                }
                self.i += 2;
                let Tok::Str(text) = string.tok else {
                    unreachable!("a string was peeked")
                };
                let text = typed_text(literal, &text, false).ok_or_else(SqlError::bad_literal)?;
                ExprKind::Typed(literal, text)
            }
            // `X'2A 2B'`: the quote must follow at once here too.
            Tok::Ident(ref word)
                if (word.eq_ignore_ascii_case("X") || word.eq_ignore_ascii_case("BINARY"))
                    && matches!(self.peek_at(1), Some(Tok::Str(_))) =>
            {
                let string = self.tokens[self.i + 1].clone();
                if string.pos != token.end {
                    return Err(self.stray_text(&string));
                }
                self.i += 2;
                let Tok::Str(text) = string.tok else {
                    unreachable!("a string was peeked")
                };
                ExprKind::Bin(hex_bytes(&text).ok_or_else(SqlError::bad_literal)?)
            }
            Tok::Ident(ref name) | Tok::Quoted(ref name) => {
                let name = name.clone();
                self.i += 1;
                let quoted = matches!(token.tok, Tok::Quoted(_));
                if quoted && self.at_punct("(") {
                    let mut error = SqlError::unexpected(
                        "Scalar function name should not be delimited.",
                        self.tokens.get(self.i + 1).map_or(self.end, |t| t.pos),
                    );
                    error.message.push_str(&self.trailer());
                    return Err(error);
                }
                if !quoted && self.at_punct("(") {
                    self.i += 1;
                    return self.call(name, pos);
                }
                if self.at_punct(".")
                    && matches!(self.peek_at(1), Some(Tok::Ident(_) | Tok::Quoted(_)))
                {
                    self.i += 1;
                    let column = self.name().unwrap();
                    // `package.function(args)`: a call to a function in a
                    // package, not a qualified column.
                    if self.at_punct("(") {
                        self.i += 1;
                        return self.call(format!("{name}.{column}"), pos);
                    }
                    ExprKind::Column {
                        table: Some(name),
                        name: column,
                    }
                } else {
                    ExprKind::Column { table: None, name }
                }
            }
            Tok::Punct(_) => return Err(self.expected("identifier or expression")),
        };
        let expr = Expr { kind, pos };
        // Only a column or a string takes a collation.
        if matches!(expr.kind, ExprKind::Str(_) | ExprKind::Column { .. })
            && self.at_word("COLLATE")
        {
            return self.collate(expr);
        }
        Ok(expr)
    }

    /// `COLLATE ads_default_ci | ads_default_cs` after `expr`.
    fn collate(&mut self, expr: Expr) -> Result<Expr, SqlError> {
        self.i += 1;
        let pos = expr.pos;
        let Some(Token {
            tok: Tok::Ident(language),
            pos: at,
            ..
        }) = self.peek().cloned()
        else {
            return Err(SqlError::expected_explained(
                " expecting language after COLLATE clause.",
                &self.trailer(),
                0,
            ));
        };
        let ci = match language.to_ascii_lowercase().as_str() {
            "ads_default_ci" => true,
            "ads_default_cs" => false,
            _ => {
                return Err(SqlError::expected(
                    " unknown language found after COLLATE clause.",
                    at,
                    &self.trailer(),
                ));
            }
        };
        self.i += 1;
        Ok(Expr {
            kind: ExprKind::Collate {
                expr: Box::new(expr),
                ci,
            },
            pos,
        })
    }

    /// `{d '…'}`, `{t '…'}`, `{ts '…'}`, `{fn f(…)}`.
    fn escape(&mut self) -> Result<Expr, SqlError> {
        let pos = self.pos();
        self.i += 1;
        let kind = match self.peek().map(|t| t.tok.clone()) {
            Some(tok) if tok.is("d") || tok.is("t") || tok.is("ts") => {
                self.i += 1;
                let literal = if tok.is("d") {
                    TypedLiteral::Date
                } else if tok.is("t") {
                    TypedLiteral::Time
                } else {
                    TypedLiteral::TimeStamp
                };
                let text = match self.next() {
                    Some(Token {
                        tok: Tok::Str(s), ..
                    }) => s,
                    _ => return Err(self.expected("string")),
                };
                let text =
                    typed_text(literal, &text, true).ok_or_else(|| self.malformed_escape(pos))?;
                ExprKind::Typed(literal, text)
            }
            Some(tok) if tok.is("fn") => {
                self.i += 1;
                // The original reports the function's own errors at its
                // `(` here (`fixtures/sql/claims.json`).
                let paren = self
                    .tokens
                    .get(self.i + 1)
                    .filter(|t| matches!(t.tok, Tok::Punct("(")))
                    .map(|t| t.pos);
                let expr = self.primary()?;
                if let (Some(paren), ExprKind::Func { .. }) = (paren, &expr.kind) {
                    self.expect_punct("}")?;
                    return Ok(Expr {
                        kind: expr.kind,
                        pos: paren,
                    });
                }
                expr.kind
            }
            Some(Tok::Ident(_)) => return Err(self.malformed_escape(pos)),
            _ => return Err(self.expected("identifier or expression")),
        };
        self.expect_punct("}")?;
        Ok(Expr { kind, pos })
    }

    /// 2118 for the escape that begins at `start`, shown up to its first `}`.
    fn malformed_escape(&self, start: Pos) -> SqlError {
        let from = self.source.get(start..).unwrap_or_default();
        let to = from
            .iter()
            .position(|c| *c == '}')
            .map_or(from.len(), |i| i + 1);
        let raw: String = from[..to].iter().collect();
        SqlError::malformed_escape(&raw, start, &self.trailer())
    }

    fn case(&mut self, pos: Pos) -> Result<Expr, SqlError> {
        let operand = if self.at_word("WHEN") {
            None
        } else {
            Some(Box::new(self.expr()?))
        };
        let mut whens = Vec::new();
        while self.eat_word("WHEN") {
            let when = self.expr()?;
            self.expect_word("THEN")?;
            let then = self.case_result()?;
            whens.push((when, then));
        }
        if whens.is_empty() {
            return Err(self.expected("WHEN"));
        }
        let otherwise = if self.eat_word("ELSE") {
            Some(Box::new(self.case_result()?))
        } else {
            None
        };
        self.expect_word("END")?;
        Ok(Expr {
            kind: ExprKind::Case {
                operand,
                whens,
                otherwise,
            },
            pos,
        })
    }

    /// An aggregate function, after its `(`.
    fn aggregate(&mut self, upper: &str, pos: Pos) -> Result<Expr, SqlError> {
        let func = match upper {
            "COUNT" => Aggregate::Count,
            "SUM" => Aggregate::Sum,
            "AVG" => Aggregate::Avg,
            "MIN" => Aggregate::Min,
            "MAX" => Aggregate::Max,
            "GROUP_CONCAT" => Aggregate::GroupConcat,
            _ => return Err(SqlError::unsupported(upper, pos)),
        };
        if func == Aggregate::Count && self.eat_punct("*") {
            self.close_call()?;
            return Ok(Expr {
                kind: ExprKind::Aggregate {
                    func,
                    arg: None,
                    distinct: false,
                    separator: None,
                    order: Vec::new(),
                },
                pos,
            });
        }
        // `COUNT` and `GROUP_CONCAT` take `DISTINCT`; after the others, the
        // original engine takes the word for a column and stumbles on what
        // follows.
        let distinct = self.at_word("DISTINCT")
            && !matches!(self.peek_at(1), None | Some(Tok::Punct(")" | ",")));
        if distinct {
            self.i += 1;
            if !matches!(func, Aggregate::Count | Aggregate::GroupConcat) {
                return Err(self.expected("identifier or expression"));
            }
        }
        let arg = self.expr()?;
        let (separator, order) = if func == Aggregate::GroupConcat {
            self.group_concat_tail()?
        } else {
            (None, Vec::new())
        };
        self.close_call()?;
        Ok(Expr {
            kind: ExprKind::Aggregate {
                func,
                arg: Some(Box::new(arg)),
                distinct,
                separator,
                order,
            },
            pos,
        })
    }

    /// `GROUP_CONCAT`'s optional `ORDER BY <expr> [ASC|DESC], …` and
    /// `SEPARATOR '<string>'`, after its argument and before the `)`.
    fn group_concat_tail(&mut self) -> Result<(Option<String>, Vec<OrderItem>), SqlError> {
        let mut order = Vec::new();
        if self.eat_word("ORDER") {
            self.expect_word("BY")?;
            loop {
                let key = OrderKey::Expr(self.expr()?);
                let descending = if self.eat_word("DESC") {
                    true
                } else {
                    self.eat_word("ASC");
                    false
                };
                order.push(OrderItem { key, descending });
                if !self.eat_punct(",") {
                    break;
                }
            }
        }
        let separator = if self.eat_word("SEPARATOR") {
            match self.next() {
                Some(Token {
                    tok: Tok::Str(s), ..
                }) => Some(s),
                _ => return Err(self.expected("a string")),
            }
        } else {
            None
        };
        Ok((separator, order))
    }

    /// A function call, after its `(`.
    fn call(&mut self, name: String, pos: Pos) -> Result<Expr, SqlError> {
        self.in_call += 1;
        let call = self.call_arguments(name, pos);
        self.in_call -= 1;
        call
    }

    /// The error of a text that follows a word with a blank between: in a
    /// condition the word is a column and the text no part of it, elsewhere
    /// the original engine takes the text for the start of a condition.
    fn stray_text(&self, string: &Token) -> SqlError {
        match self.context {
            Context::Where | Context::DeleteWhere | Context::UpdateWhere | Context::Having => {
                SqlError::unexpected_token("", string.pos)
            }
            _ => SqlError::expected_explained(
                "Unexpected token while parsing for Boolean expression.",
                &self.trailer(),
                string.pos,
            ),
        }
    }

    /// The `)` that ends a call's arguments. The original engine places its
    /// absence at the token before the operand the call is in, and takes a
    /// literal after an argument for the start of a condition
    /// (`fixtures/sql/gram_syntax.json`).
    fn close_call(&mut self) -> Result<(), SqlError> {
        if self.eat_punct(")") {
            return Ok(());
        }
        let at = self.anchor.unwrap_or_else(|| self.pos_expected());
        let what = match self.at_literal() {
            true => "Unexpected token while parsing for Boolean expression.",
            false => ")",
        };
        Err(SqlError::expected_explained(what, &self.trailer(), at))
    }

    fn call_arguments(&mut self, name: String, pos: Pos) -> Result<Expr, SqlError> {
        let upper = name.to_ascii_uppercase();
        if upper == "EXISTS" && self.eat_word("SELECT") {
            let select = self.subselect()?;
            return Ok(Expr {
                kind: ExprKind::Exists(Box::new(select)),
                pos,
            });
        }
        if AGGREGATES.contains(&upper.as_str()) {
            return self.aggregate(&upper, pos);
        }
        let kind = match upper.as_str() {
            "CAST" => {
                let expr = self.argument()?;
                self.expect_word("AS")?;
                let to = self.cast_type("CAST", pos)?;
                ExprKind::Cast {
                    expr: Box::new(expr),
                    to,
                    function: "CAST",
                }
            }
            "CONVERT" => {
                let expr = self.argument()?;
                self.expect_punct(",")?;
                let to = self.cast_type("CONVERT", pos)?;
                ExprKind::Cast {
                    expr: Box::new(expr),
                    to,
                    function: "CONVERT",
                }
            }
            "EXTRACT" => {
                let part = self.name().ok_or_else(|| self.expected("<identifier>"))?;
                self.expect_word("FROM")?;
                let expr = self.expr()?;
                ExprKind::Extract {
                    part: part.to_ascii_uppercase(),
                    expr: Box::new(expr),
                }
            }
            "TRIM" => {
                let side = if self.eat_word("LEADING") {
                    Some(TrimSide::Leading)
                } else if self.eat_word("TRAILING") {
                    Some(TrimSide::Trailing)
                } else if self.eat_word("BOTH") {
                    Some(TrimSide::Both)
                } else {
                    None
                };
                let first = if side.is_some() && self.at_word("FROM") {
                    None
                } else {
                    Some(self.expr()?)
                };
                if self.eat_word("FROM") {
                    let expr = self.expr()?;
                    ExprKind::Trim {
                        side: side.unwrap_or(TrimSide::Both),
                        chars: first.map(Box::new),
                        expr: Box::new(expr),
                    }
                } else {
                    let expr = first.ok_or_else(|| self.expected("FROM"))?;
                    ExprKind::Trim {
                        side: side.unwrap_or(TrimSide::Both),
                        chars: None,
                        expr: Box::new(expr),
                    }
                }
            }
            "POSITION" => {
                let needle = self.bitwise()?;
                self.expect_word("IN")?;
                let haystack = self.expr()?;
                ExprKind::Position(Box::new(needle), Box::new(haystack))
            }
            "CONTAINS" => {
                let all = self.eat_punct("*");
                let field = if all {
                    None
                } else {
                    Some(Box::new(self.expr()?))
                };
                self.expect_punct(",")?;
                let condition = Box::new(self.argument()?);
                ExprKind::Contains {
                    all,
                    field,
                    condition,
                }
            }
            "SCORE" | "SCOREDISTINCT" => {
                let distinct = upper == "SCOREDISTINCT";
                let spec = if self.eat_punct("*") {
                    self.expect_punct(",")?;
                    ScoreSpec::Field {
                        all: true,
                        field: None,
                        condition: Box::new(self.argument()?),
                    }
                } else {
                    let first = self.expr()?;
                    if self.eat_punct(",") {
                        let condition = Box::new(self.argument()?);
                        ScoreSpec::Field {
                            all: false,
                            field: Some(Box::new(first)),
                            condition,
                        }
                    } else if let ExprKind::Number(n) = &first.kind
                        && n.bytes().all(|b| b.is_ascii_digit())
                    {
                        ScoreSpec::Ordinal(n.parse().unwrap_or(i64::MAX))
                    } else {
                        return Err(self.expected("',' or a score number"));
                    }
                };
                ExprKind::Score { distinct, spec }
            }
            // One expression, and the unit of its length after `USING`.
            "CHAR_LENGTH" | "CHARACTER_LENGTH" if !self.at_punct(")") => {
                let trailer = self.trailer();
                // The expression may be missing, which its arguments' count
                // refuses, or be `NULL`.
                let mut args = Vec::new();
                if self.at_word("NULL") && self.at_word_at(1, "USING") {
                    args.push(Expr {
                        kind: ExprKind::Null,
                        pos: self.pos(),
                    });
                    self.i += 1;
                } else if !self.at_word("USING") {
                    args.push(self.argument()?);
                }
                if self.eat_word("USING") {
                    const UNITS: [&str; 3] = ["CHARACTERS", "OCTETS", "CODE_POINTS"];
                    let unit = match self.peek().map(|t| &t.tok) {
                        Some(Tok::Ident(word))
                            if UNITS.iter().any(|u| word.eq_ignore_ascii_case(u)) =>
                        {
                            Some(word.to_ascii_uppercase())
                        }
                        _ => None,
                    };
                    let Some(unit) = unit else {
                        let what = match self.peek().map(|t| &t.tok) {
                            Some(Tok::Ident(_)) => "",
                            _ => "<keyword>",
                        };
                        return Err(SqlError::expected_using(what, self.grant_pos(), &trailer));
                    };
                    self.i += 1;
                    // Without an expression there is no unit to tell of.
                    if !args.is_empty() {
                        args.push(Expr {
                            kind: ExprKind::Str(unit),
                            pos: self.pos(),
                        });
                    }
                }
                if !self.at_punct(")") {
                    return Err(SqlError::expected_explained(")", &trailer, 0));
                }
                ExprKind::Func { name, args }
            }
            // These take a bare word, not an expression.
            "RECNO" | "DELETED" | "LASTAUTOINC" if !self.at_punct(")") => {
                // The original engine words these errors with the place of
                // the statement's start.
                let at = self.pos();
                let trailer = self.trailer();
                let refused = |what: &str| SqlError::expected_explained(what, &trailer, 0);
                let word = match self.name() {
                    Some(word) if upper != "LASTAUTOINC" => word,
                    Some(word)
                        if word.eq_ignore_ascii_case("CONNECTION")
                            || word.eq_ignore_ascii_case("STATEMENT") =>
                    {
                        word
                    }
                    _ if upper == "LASTAUTOINC" => {
                        return Err(refused("CONNECTION or STATEMENT"));
                    }
                    _ => return Err(refused(")")),
                };
                if !self.at_punct(")") {
                    return Err(refused(")"));
                }
                ExprKind::Func {
                    name,
                    args: vec![Expr {
                        kind: ExprKind::Column {
                            table: None,
                            name: word,
                        },
                        pos: at,
                    }],
                }
            }
            _ => {
                let open = self.tokens[self.i - 1].pos;
                let mut args = Vec::new();
                if !self.at_punct(")") {
                    loop {
                        args.push(self.argument()?);
                        if !self.eat_punct(",") {
                            break;
                        }
                    }
                }
                // Only `CHAR_LENGTH` takes a unit; the original engine words
                // this missing `)` as the statement's start.
                if self.at_word("USING")
                    && ["LEN", "LENGTH", "OCTET_LENGTH"].contains(&upper.as_str())
                {
                    return Err(SqlError::expected_explained(")", &self.trailer(), 0));
                }
                // The precision is read where the call is parsed, so that
                // the error has the place of the `(` and of the column.
                if upper == "CURRENT_TIMESTAMP" {
                    let reason = match &args[..] {
                        [] => None,
                        [
                            Expr {
                                kind: ExprKind::Number(n),
                                ..
                            },
                        ] if matches!(n.as_str(), "0" | "1" | "2" | "3") => None,
                        [_] => Some(
                            ". The specified precision must be an integer numeric literal \
                             between 0 and 3.",
                        ),
                        _ => Some(" Incorrect number of parameters"),
                    };
                    if let Some(reason) = reason {
                        return Err(SqlError::bad_argument_why(
                            &upper,
                            reason,
                            open,
                            &self.trailer(),
                        ));
                    }
                }
                ExprKind::Func { name, args }
            }
        };
        self.close_call()?;
        Ok(Expr { kind, pos })
    }

    /// The type of `CAST` and `CONVERT` (`function`, at `call`): `SQL_INTEGER`,
    /// `SQL_CHAR(3)`, … Only the names that begin with `SQL_` are types, and
    /// the original engine words what is wrong with a size one type takes
    /// otherwise, or none (`fixtures/sql/gram_cast3.json`).
    fn cast_type(&mut self, function: &str, call: Pos) -> Result<CastType, SqlError> {
        // The original engine's own parser of the type reads the place of
        // its error as the start of the expression.
        let malformed =
            |this: &Self, what: &str| SqlError::expected_explained(what, &this.trailer(), 0);
        let Some(name) = self.name() else {
            return Err(malformed(self, "<identifier>"));
        };
        let upper = name.to_ascii_uppercase();
        if upper == "VARCHAR" {
            return Err(malformed(self, "<identifier>"));
        }
        let mut sizes = Vec::new();
        if self.eat_punct("(") {
            if self.at_punct("-") {
                return Err(SqlError::bad_argument(function, call));
            }
            loop {
                match self.peek().map(|t| &t.tok) {
                    Some(Tok::Number(n)) if n.bytes().all(|b| b.is_ascii_digit()) => {
                        // A size is kept in 16 bits and wraps there; one past
                        // 32 bits is refused with no reason
                        // (`fixtures/sql/gram_syntax.json`).
                        let Ok(size) = n.parse::<u32>() else {
                            return Err(SqlError::bad_argument(function, call));
                        };
                        sizes.push(size as u16);
                        self.i += 1;
                        if sizes.len() == 2 && self.at_punct(",") {
                            return Err(malformed(self, ")"));
                        }
                    }
                    _ => return Err(malformed(self, "<number>")),
                }
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct(")")?;
        }
        let unrecognized = || {
            SqlError::bad_argument_why(
                function,
                &format!(" - unrecognized data type: {name}."),
                call,
                "",
            )
        };
        let Some(base) = upper.strip_prefix("SQL_") else {
            return Err(unrecognized());
        };
        // ODBC 3's names of the date and time types.
        let base = match base {
            "TYPE_DATE" => "DATE",
            "TYPE_TIME" => "TIME",
            "TYPE_TIMESTAMP" => "TIMESTAMP",
            other => other,
        };
        let why = |why: &str| SqlError::bad_argument_why(function, &format!(" - {why}."), call, "");
        let length = sizes.first().copied();
        // A size is the length of text and binary, the precision and the
        // scale of a NUMERIC, and no part of any other type.
        let sized = matches!(
            base,
            "CHAR" | "VARCHAR" | "WCHAR" | "WVARCHAR" | "BINARY" | "VARBINARY"
        );
        if !sizes.is_empty() && base != "NUMERIC" {
            if !sized {
                if !matches!(
                    base,
                    "LONGVARCHAR"
                        | "WLONGVARCHAR"
                        | "LONGVARBINARY"
                        | "BIT"
                        | "TINYINT"
                        | "SMALLINT"
                        | "INTEGER"
                        | "BIGINT"
                        | "DECIMAL"
                        | "FLOAT"
                        | "DOUBLE"
                        | "REAL"
                        | "MONEY"
                        | "DATE"
                        | "TIME"
                        | "TIMESTAMP"
                        | "GUID"
                ) {
                    return Err(unrecognized());
                }
                return Err(why(
                    "precision should not be specified for the type converted to",
                ));
            }
            if sizes.len() > 1 {
                return Err(why("scale is not valid for this type"));
            }
            if length == Some(0) {
                return Err(why("precision cannot be zero"));
            }
            // The wide types take two bytes a character.
            let most = if matches!(base, "WCHAR" | "WVARCHAR") {
                32500
            } else {
                65530
            };
            if length.is_some_and(|n| n > most) {
                return Err(why("length exceeds maximum allowed"));
            }
        }
        let precision = match sizes[..] {
            [_] if base == "NUMERIC" => return Err(why("must specify both precision and scale")),
            [p, s, ..] => Some((p, s)),
            _ => None,
        };
        Ok(match base {
            "CHAR" => CastType::Char(length),
            "VARCHAR" => CastType::VarChar(length),
            "LONGVARCHAR" => CastType::LongVarChar,
            "WCHAR" => CastType::WChar(length),
            "WVARCHAR" => CastType::WVarChar(length),
            "WLONGVARCHAR" => CastType::WLongVarChar,
            "BINARY" => CastType::Binary(length),
            "VARBINARY" => CastType::VarBinary(length),
            // Not one to make (`fixtures/sql/gram_cast3.json`).
            "LONGVARBINARY" => return Err(SqlError::not_supported()),
            "BIT" => CastType::Bit,
            "TINYINT" => CastType::TinyInt,
            "SMALLINT" => CastType::SmallInt,
            "INTEGER" => CastType::Integer,
            "BIGINT" => CastType::BigInt,
            "NUMERIC" => CastType::Numeric(precision),
            "DECIMAL" => CastType::Decimal(None),
            "FLOAT" => CastType::Float,
            "DOUBLE" => CastType::Double,
            "REAL" => CastType::Real,
            "MONEY" => CastType::Money,
            "DATE" => CastType::Date,
            "TIME" => CastType::Time,
            "TIMESTAMP" => CastType::TimeStamp,
            "GUID" => CastType::Guid,
            _ => return Err(unrecognized()),
        })
    }
}

/// The words that end a sequence of script statements.
const BLOCK_ENDS: [&str; 7] = [
    "END", "ELSE", "ELSEIF", "ELIF", "ENDIF", "ENDWHILE", "ENDTRY",
];

/// The bytes of the hex digits of a binary literal, where spaces between
/// them are no part of it (`fixtures/sql/gram_bin.json`); `None` for any
/// other character or an odd number of digits.
fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    let digits: Vec<u32> = text
        .chars()
        .filter(|&c| c != ' ')
        .map(|c| c.to_digit(16))
        .collect::<Option<_>>()?;
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    Some(digits.chunks(2).map(|d| (d[0] * 16 + d[1]) as u8).collect())
}

fn binary(op: BinOp, left: Expr, right: Expr) -> Expr {
    let pos = left.pos;
    Expr {
        kind: ExprKind::Binary(op, Box::new(left), Box::new(right)),
        pos,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select(text: &str) -> Select {
        match parse(text).unwrap().statement {
            Statement::Select(select) => *select,
            other => panic!("not a select: {other:?}"),
        }
    }

    fn error(text: &str) -> (u32, String) {
        let e = parse(text).unwrap_err();
        (e.native, e.message)
    }

    #[test]
    fn a_trigger_can_be_a_function_of_a_library() {
        let trigger = |text: &str| match parse(text).unwrap().statement {
            Statement::CreateTrigger(create) => *create,
            other => panic!("not a trigger: {other:?}"),
        };
        let create = trigger(
            "CREATE TRIGGER t ON x AFTER UPDATE FUNCTION Fn IN LIBRARY \"a.dll\" NO MEMOS \
             PRIORITY 3;",
        );
        assert_eq!(
            create.library,
            Some(TriggerLibrary {
                function: "Fn".into(),
                library: "a.dll".into(),
                assembly: false
            })
        );
        assert_eq!((create.options, create.priority, create.event), (1, 3, 2));
        let create =
            trigger("ALTER TRIGGER t ON x BEFORE DELETE FUNCTION Fn IN ASSEMBLY [a.b] NO VALUES");
        assert_eq!(create.options, 0);
        assert!(create.library.unwrap().assembly);
    }

    #[test]
    fn selects() {
        let s = select(
            "SELECT TOP 2 START AT 3 DISTINCT a, b AS x, c y FROM t z WHERE a > 1 ORDER BY 2 DESC, a;",
        );
        assert_eq!(
            s.top,
            Some(Top {
                count: 2,
                start: 3,
                percent: false
            })
        );
        assert!(s.distinct);
        assert_eq!(s.items.len(), 3);
        assert_eq!(s.table().unwrap().alias.as_deref(), Some("z"));
        assert!(s.filter.is_some());
        assert_eq!(s.order.len(), 2);
        assert!(select("SELECT {static} * FROM system.iota").force_static);
        assert_eq!(
            select("SELECT * FROM system.iota").table().unwrap().name,
            "system.iota"
        );
    }

    #[test]
    fn select_into_and_procedures_in_from() {
        let s = select("SELECT a INTO t2 FROM t UNION SELECT b FROM u");
        assert_eq!(s.into.unwrap().name, "t2");
        // Only the first `SELECT` of a statement takes the table.
        for text in [
            "SELECT a FROM t UNION SELECT b INTO t2 FROM u",
            "SELECT * FROM (SELECT a INTO t2 FROM t) x",
            "SELECT a INTO t2, t3 FROM t",
        ] {
            assert_eq!(error(text).0, 2115, "{text}");
        }
        assert!(error("SELECT a INTO FROM t").1.contains("<identifier>"));
        let s = select("SELECT * FROM (EXECUTE PROCEDURE p(1, 'x')) q, t");
        let From::Join { left, .. } = &s.from else {
            panic!()
        };
        let From::Procedure { execute, alias, .. } = &**left else {
            panic!()
        };
        assert_eq!((execute.name.as_str(), execute.args.len()), ("p", 2));
        assert_eq!(alias.as_deref(), Some("q"));
        assert_eq!(error("SELECT * FROM (EXECUTE PROCEDURE p())").0, 2125);
        assert_eq!(error("SELECT * FROM (EXECUTE p()) q").0, 2115);
        assert_eq!(error("SELECT * FROM EXECUTE PROCEDURE p() q").0, 2115);
    }

    #[test]
    fn group_concat() {
        let agg = |text: &str| {
            let s = select(&format!("SELECT {text} FROM t"));
            let Item::Expr { expr, .. } = &s.items[0] else {
                panic!()
            };
            let ExprKind::Aggregate {
                func,
                distinct,
                separator,
                order,
                ..
            } = expr.kind.clone()
            else {
                panic!("not an aggregate: {expr:?}")
            };
            (func, distinct, separator, order.len())
        };
        assert_eq!(
            agg("GROUP_CONCAT(a)"),
            (Aggregate::GroupConcat, false, None, 0)
        );
        assert_eq!(
            agg("GROUP_CONCAT(DISTINCT a SEPARATOR '; ')"),
            (Aggregate::GroupConcat, true, Some("; ".to_owned()), 0)
        );
        assert_eq!(agg("GROUP_CONCAT(a ORDER BY b, c DESC)").3, 2);
        // Only COUNT and GROUP_CONCAT take DISTINCT.
        assert!(parse("SELECT SUM(DISTINCT a) FROM t").is_err());
        // SEPARATOR wants a string.
        assert!(parse("SELECT GROUP_CONCAT(a SEPARATOR 1) FROM t").is_err());
    }

    #[test]
    fn precedence() {
        let s = select("SELECT 1 + 2 * 3 FROM t");
        let Item::Expr { expr, .. } = &s.items[0] else {
            panic!()
        };
        let ExprKind::Binary(BinOp::Add, _, right) = &expr.kind else {
            panic!("{expr:?}")
        };
        assert!(matches!(right.kind, ExprKind::Binary(BinOp::Mul, _, _)));
        let s = select("SELECT * FROM t WHERE NOT a = 1 OR b = 2");
        let ExprKind::Binary(BinOp::Or, left, _) = &s.filter.as_ref().unwrap().kind else {
            panic!()
        };
        assert!(matches!(left.kind, ExprKind::Not(_)));
    }

    #[test]
    fn params() {
        let parsed = parse("SELECT * FROM t WHERE a = :x AND b = ? AND c = :X").unwrap();
        // A name that comes again is a parameter of its own.
        assert_eq!(
            parsed.params,
            vec![Some("x".to_owned()), None, Some("X".to_owned())]
        );
    }

    #[test]
    fn package_qualified_function_call() {
        // `pkg.func(args)` is a call to a function in a package, not a column.
        let parsed = parse("SELECT util.pad(id, 4) AS p FROM t").unwrap();
        let Statement::Select(select) = &parsed.statement else {
            panic!("select")
        };
        let Item::Expr { expr, .. } = &select.items[0] else {
            panic!("item")
        };
        let ExprKind::Func { name, args } = &expr.kind else {
            panic!("func: {:?}", expr.kind)
        };
        assert_eq!(name, "util.pad");
        assert_eq!(args.len(), 2);
        // A qualified column with no call is still a column.
        let parsed = parse("SELECT t.id FROM t").unwrap();
        let Statement::Select(select) = &parsed.statement else {
            panic!("select")
        };
        let Item::Expr { expr, .. } = &select.items[0] else {
            panic!("item")
        };
        assert!(
            matches!(&expr.kind, ExprKind::Column { table: Some(t), name } if t == "t" && name == "id")
        );
    }

    #[test]
    fn errors_match_the_original_engine() {
        assert_eq!(error("SELEC * FROM t").0, 2115);
        assert_eq!(
            error("SELECT * FROM indexed garbage garbage").1,
            "Unexpected token: garbage -- Expecting semicolon. -- Location of error in the SQL \
             statement is: 31"
        );
        assert_eq!(
            error("SELECT * FROM indexed WHERE").1,
            "Expected lexical element not found: identifier or expression -- Location of error \
             in the SQL statement is: 27 There was a problem parsing the WHERE clause."
        );
        assert_eq!(
            error("SELECT").1,
            "Expected lexical element not found: identifier or expression -- Location of error \
             in the SQL statement is: 6 [Parsing Expression (column 1 in the SELECT clause)]"
        );
        assert_eq!(
            error("SELECT *, id FROM indexed").1,
            "Expected lexical element not found: FROM Missing the keyword FROM after the column \
             definitions in the SELECT statement. -- Location of error in the SQL statement is: 9"
        );
        assert_eq!(error("SELECT 'x").0, 2113);
        assert_eq!(
            error("SELECT NULL FROM t").1.rsplit(": ").next(),
            Some("13 [Parsing Expression (column 1 in the SELECT clause)]")
        );
    }
    #[test]
    fn parses_merge() {
        let sql = "MERGE INTO t a USING s b ON a.id = b.id WHEN NOT MATCHED THEN INSERT \
                   WITH DELETE (id) VALUES (b.id, DEFAULT) WHEN MATCHED THEN UPDATE WITH \
                   RECALL SET a.v = NULL, w = 1";
        let Statement::Merge(merge) = parse(sql).unwrap().statement else {
            panic!("merge")
        };
        assert_eq!(merge.table.alias.as_deref(), Some("a"));
        assert!(matches!(merge.source, Some(From::Table(_))));
        let update = merge.update.unwrap();
        assert_eq!(update.mark, Mark::Recall);
        assert_eq!(update.sets[0].qualifier.as_deref(), Some("a"));
        assert!(merge.insert.unwrap().deleted);
        let Statement::Merge(merge) =
            parse("MERGE t ON id = 1 WHEN NOT MATCHED THEN INSERT DEFAULT VALUES")
                .unwrap()
                .statement
        else {
            panic!("merge")
        };
        assert!(merge.source.is_none() && merge.update.is_none());
        assert_eq!(parse("MERGE t ON id = 1").unwrap_err().native, 2115);
    }

    #[test]
    fn views_keep_the_text_of_their_select() {
        let Statement::CreateView(view) =
            parse("CREATE VIEW v AS /* a */ SELECT id,\r\n  name FROM t -- b\r\n;")
                .unwrap()
                .statement
        else {
            panic!("not a view");
        };
        assert_eq!(view.text, "SELECT id,\r\n  name FROM t");
        let checked = check_view("v", " select id from t ; ").unwrap();
        assert_eq!(checked.text, "select id from t");
    }

    #[test]
    fn views_sort_only_with_top() {
        let (native, message) = error("CREATE VIEW v AS SELECT id FROM t ORDER BY id");
        assert_eq!(native, 2117);
        assert!(message.ends_with("is: 35"), "{message}");
        // A derived table or a subquery is a missing `)`, as many times as
        // it is nested, and `TOP` allows an `ORDER BY`.
        let (native, message) =
            error("CREATE VIEW v AS SELECT * FROM (SELECT id FROM t ORDER BY id) x");
        assert_eq!(native, 2115);
        assert_eq!(
            message,
            "Expected lexical element not found: ) There was a problem parsing the table names \
             after the FROM keyword. There was an error processing the view SELECT statement \
             -- Location of error in the SQL statement is: 50"
        );
        assert!(
            parse("CREATE VIEW v AS SELECT * FROM (SELECT TOP 1 id FROM t ORDER BY id) x").is_ok()
        );
        let nested = check_view(
            "v",
            "SELECT * FROM (SELECT * FROM (SELECT id FROM t ORDER BY id) z) x",
        )
        .unwrap_err();
        assert_eq!(nested.native, 2115);
        assert_eq!(nested.message.matches("FROM keyword").count(), 2);
        let in_where = check_view(
            "v",
            "SELECT id FROM t WHERE id IN (SELECT id FROM u ORDER BY id)",
        );
        assert!(in_where.unwrap_err().message.contains("WHERE clause"));
    }

    #[test]
    fn procedures() {
        let Statement::CreateProcedure(create) = parse(
            "ALTER PROCEDURE p(a INTEGER, b CHAR(4), CURSOR VARYING OUTPUT) \
             BEGIN \r\n  SELECT 1 FROM system.iota; END;",
        )
        .unwrap()
        .statement
        else {
            panic!("not a procedure");
        };
        assert!(create.alter && create.varying);
        assert_eq!(create.params.len(), 2);
        // The script keeps its text but the first line break.
        assert_eq!(
            create.body,
            ProcBody::Script("  SELECT 1 FROM system.iota; ".to_owned())
        );
        let (native, message) =
            error("CREATE PROCEDURE p(a INTEGER, a CHAR(2)) BEGIN SELECT 1 FROM system.iota; END");
        assert_eq!(native, 2217);
        assert!(
            message.contains("Input parameter name already used: a"),
            "{message}"
        );
        assert_eq!(
            error("CREATE PROCEDURE p() BEGIN SELECT 1 FROM system.iota END").0,
            2117
        );
        assert_eq!(
            error("CREATE PROCEDURE p() BEGIN SELECT 1 FROM system.iota;").0,
            2216
        );
        assert_eq!(error("CREATE PROCEDURE p() BEGIN END").0, 2115);
        assert_eq!(
            error("CREATE PROCEDURE \"p\"() BEGIN SELECT 1 FROM system.iota; END").0,
            2115
        );
        assert_eq!(
            error("CREATE PROCEDURE p(CURSOR VARYING OUTPUT) FUNCTION f IN LIBRARY \"l\"").0,
            2215
        );
        let Statement::CreateProcedure(external) =
            parse("CREATE PROCEDURE p(a INTEGER) FUNCTION f IN COMLIBRARY lib")
                .unwrap()
                .statement
        else {
            panic!("not a procedure");
        };
        assert_eq!(
            external.body,
            ProcBody::External {
                function: "f".into(),
                library: "lib".into(),
                com: true,
            }
        );
    }

    #[test]
    fn cache_prepare() {
        for text in [
            "CACHE PREPARE ON",
            "cache prepare off;",
            "CACHE PREPARE DEFAULT; SELECT 1 FROM t;",
        ] {
            assert!(parse(text).is_ok(), "{text}");
        }
        // The original words `CACHE` alone without a location.
        assert_eq!(
            error("CACHE").1,
            "Expected lexical element not found: PREPARE"
        );
        assert_eq!(error("CACHE PREPARE").0, 2115);
        assert!(error("CACHE PREPARE ONN").1.contains("Expect ON, OFF"));
        assert_eq!(error("CACHE PREPARE ON junk").0, 2117);
    }

    #[test]
    fn ddl_of_what_is_not_there() {
        // The rest are its syntax errors, placed as it places them.
        for (text, native, at) in [
            ("CREATE USER u", 2117, 8),
            ("CREATE", 2117, 1),
            ("CREATE UNIQUE", 2117, 8),
            ("DROP USER u", 2117, 11),
            ("DROP USER", 2117, 9),
            ("DROP 1", 2117, 6),
            ("ALTER VIEW v", 2115, 7),
            ("ALTER PACKAGE p", 2115, 7),
            ("ALTER ONLINE USER", 2115, 14),
        ] {
            let (code, message) = error(text);
            assert_eq!(code, native, "{text}");
            assert!(message.ends_with(&format!("is: {at}")), "{text}: {message}");
        }
    }

    #[test]
    fn functions_and_packages() {
        let Statement::FunctionDdl(ddl) = parse(
            "CREATE FUNCTION pk.f( a  INTEGER ,\r\n b Char(3) ) RETURNS Char( 20 ) \
             DESCRIPTION 'it''s' BEGIN\r\n RETURN a; END;",
        )
        .unwrap()
        .statement
        else {
            panic!("not a function");
        };
        let FunctionDdl::Create(create) = *ddl else {
            panic!("not a create");
        };
        assert_eq!(create.package.unwrap().name, "pk");
        assert_eq!(create.name.name, "f");
        assert_eq!(create.params, "a  INTEGER ,\r\n b Char(3)");
        assert_eq!(
            (create.param_count, create.returns.as_str()),
            (2, "Char ( 20 )")
        );
        assert_eq!(create.description.as_deref(), Some("it's"));
        assert_eq!(create.body, " RETURN a; ");
        // A new name, which may name the package again.
        let Statement::FunctionDdl(ddl) =
            parse("ALTER FUNCTION pk.f PK.g() RETURNS DOUBLE BEGIN RETURN 1; END")
                .unwrap()
                .statement
        else {
            panic!("not a function");
        };
        let FunctionDdl::Create(create) = *ddl else {
            panic!("not a create");
        };
        assert!(create.alter);
        assert_eq!(create.new_name.unwrap().name, "g");
        assert_eq!(create.returns, "DOUBLE ( 15 )");
        for text in [
            "DROP FUNCTION f",
            "DROP FUNCTION p.f;",
            "CREATE PACKAGE p",
            "DROP PACKAGE p",
        ] {
            assert!(matches!(
                parse(text).unwrap().statement,
                Statement::FunctionDdl(_)
            ));
        }
        // What the original refuses of them (`fixtures/sql/ddfuncs`).
        for (text, native, at) in [
            (
                "CREATE FUNCTION upper() RETURNS INTEGER BEGIN RETURN 1; END",
                2230,
                17,
            ),
            (
                "CREATE FUNCTION f() RETURNS CURSOR BEGIN RETURN 1; END",
                2230,
                29,
            ),
            (
                "CREATE FUNCTION f(a CURSOR) RETURNS INTEGER BEGIN RETURN 1; END",
                2230,
                19,
            ),
            (
                "CREATE FUNCTION f(a NOSUCH) RETURNS INTEGER BEGIN RETURN 1; END",
                2135,
                19,
            ),
            (
                "CREATE FUNCTION f(a INTEGER, a INTEGER) RETURNS INTEGER BEGIN RETURN 1; END",
                2217,
                30,
            ),
            ("CREATE FUNCTION f(a INTEGER) BEGIN RETURN 1; END", 2115, 30),
            (
                "ALTER FUNCTION f pk.g() RETURNS INTEGER BEGIN RETURN 1; END",
                2230,
                18,
            ),
            (
                "ALTER FUNCTION a.f b.g() RETURNS INTEGER BEGIN RETURN 1; END",
                2230,
                20,
            ),
            ("CREATE PACKAGE", 2115, 14),
            ("DROP PACKAGE", 2115, 12),
        ] {
            let (code, message) = error(text);
            assert_eq!(code, native, "{text}: {message}");
            assert!(message.ends_with(&format!("is: {at}")), "{text}: {message}");
        }
    }

    #[test]
    fn databases() {
        let Statement::CreateDatabase(create) = parse(
            "CREATE DATABASE \"dc\\a.add\" DESCRIPTION 'd' ENCRYPT FALSE ENCRYPTIONTYPE RC4;",
        )
        .unwrap()
        .statement
        else {
            panic!("not a database");
        };
        assert_eq!(
            (create.name.as_str(), create.description.as_str()),
            ("dc\\a.add", "d")
        );
        // What tflip does not write is 2200.
        for text in ["CREATE DATABASE d PASSWORD 'pw'"] {
            assert_eq!(error(text).0, 2200, "{text}");
        }
        // An RC4 dictionary: ENCRYPT TRUE with no AES type.
        for text in [
            "CREATE DATABASE d ENCRYPT TRUE",
            "CREATE DATABASE d ENCRYPT TRUE ENCRYPTIONTYPE RC4",
        ] {
            let Statement::CreateDatabase(create) = parse(text).unwrap().statement else {
                panic!("{text}");
            };
            assert_eq!((create.aes, create.encrypt), (None, true), "{text}");
        }
        // An AES dictionary, its records encrypted or not.
        for (text, aes, encrypt) in [
            (
                "CREATE DATABASE d ENCRYPTIONTYPE AES128 DDPASSWORD 'x'",
                5,
                false,
            ),
            (
                "CREATE DATABASE d ENCRYPT TRUE ENCRYPTIONTYPE AES256 DDPASSWORD 'x'",
                6,
                true,
            ),
            (
                "CREATE DATABASE d DDPASSWORD 'x' ENCRYPTIONTYPE AES128 ENCRYPT TRUE",
                5,
                true,
            ),
        ] {
            let Statement::CreateDatabase(create) = parse(text).unwrap().statement else {
                panic!("{text}");
            };
            assert_eq!(create.aes, Some((aes, "x".to_owned())), "{text}");
            assert_eq!(create.encrypt, encrypt, "{text}");
        }
        assert_eq!(error("CREATE DATABASE d ENCRYPTIONTYPE AES256").0, 2215);
        assert_eq!(error("CREATE DATABASE d EXTRA").0, 2215);
        assert_eq!(error("CREATE DATABASE").0, 2115);
        assert!(parse("CREATE DATABASE d PASSWORD '' DDPASSWORD 'x'").is_ok());
    }
}
