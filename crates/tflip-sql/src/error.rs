//! SQL errors, as the original engine reports them: ACE error 7200 with an
//! SQL state, a native error code and a message, which `AdsGetLastError`
//! returns as
//!
//! ```text
//! Error 7200:  AQE Error:  State = S0000;   NativeError = 2121;  [SAP][Advantage SQL Engine]Column not found: nosuch -- Location of error in the SQL statement is: 8
//! ```

use std::fmt;

use serde::{Deserialize, Serialize};

/// An error of an SQL statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlError {
    /// The SQL state (`42000` for syntax errors, `S0000` for semantic ones,
    /// ...).
    pub state: String,
    /// The native error code: 2xxx for the SQL engine's own errors, or the
    /// ACE error code of a failure underneath (5004 for a missing table).
    pub native: u32,
    /// The text after `[SAP][Advantage SQL Engine]`, the location included.
    pub message: String,
}

/// Where in the statement an error is: the 0-based position of a character.
pub type Pos = usize;

/// The location suffix the original engine appends: 1-based.
pub(crate) fn at(pos: Pos) -> String {
    format!(" -- Location of error in the SQL statement is: {}", pos + 1)
}

/// What the original engine puts before where a script statement's error is.
const SCRIPT_ERROR: &str = " ** Script error information:";

/// [`SCRIPT_ERROR`] with nothing after it.
pub const SCRIPT_NOTE: &str = " ** Script error information: ";

/// The suffix of an error of a script statement at `pos`.
pub(crate) fn script_at(pos: Pos) -> String {
    format!("{SCRIPT_ERROR} {}", at(pos))
}

impl SqlError {
    fn new(state: &str, native: u32, message: String) -> SqlError {
        SqlError {
            state: state.to_owned(),
            native,
            message,
        }
    }

    /// The error, with the location `pos` it is at.
    pub(crate) fn located_at(mut self, pos: Pos) -> SqlError {
        self.message.push_str(&at(pos));
        self
    }

    /// 2224, a script's own exception (`RAISE name(code, message)`), the
    /// exception's name at `pos`.
    pub fn raised(name: &str, code: &str, message: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2224,
            format!(
                "An exception is raised in the SQL script. {{[{name}] {code} : {message}}}{}",
                script_at(pos)
            ),
        )
    }

    /// 2115, a syntax error: `what` was expected at `pos`; `trailer`
    /// follows the location.
    pub(crate) fn expected(what: &str, pos: Pos, trailer: &str) -> SqlError {
        SqlError::new(
            "42000",
            2115,
            format!(
                "Expected lexical element not found: {what}{}{trailer}",
                at(pos)
            ),
        )
    }

    /// 2115 with the explanation before the location, as the original
    /// engine words some.
    pub(crate) fn expected_explained(what: &str, explanation: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2115,
            format!(
                "Expected lexical element not found: {what}{explanation}{}",
                at(pos)
            ),
        )
    }

    /// 2230: a function definition the original refuses, `why`, at `pos`.
    pub(crate) fn invalid_function(why: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2230,
            format!("Invalid function definition: {why}{}", at(pos)),
        )
    }

    /// 2115: the unit after `USING` is none; `what` is `<keyword>` for a
    /// token that is no word, and the statement's context follows the place.
    pub(crate) fn expected_using(what: &str, pos: Pos, trailer: &str) -> SqlError {
        SqlError::new(
            "42000",
            2115,
            format!(
                "Expected lexical element not found: {what} Expecting CHARACTERS, OCTETS or \
                 CODE_POINTS after the keyword USING.{}{trailer}",
                at(pos)
            ),
        )
    }

    /// 2217: a `DECLARE` the script does not allow, `why`, its variable at
    /// `pos`.
    pub fn declaration(why: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2217,
            format!("The declaration statement is not valid: {why}{}", at(pos)),
        )
    }

    /// 2218: a script statement names a variable it did not declare.
    pub(crate) fn variable_not_found(name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2218,
            format!("The variable is not found: {name}{}", at(pos)),
        )
    }

    /// 2218: `FETCH` of a name that is not a declared cursor, at `pos`.
    pub(crate) fn cursor_not_declared(name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2218,
            format!(
                "The variable is not found: {name} : Cursor is not declared{}",
                at(pos)
            ),
        )
    }

    /// 2216: the script ends where `expecting` should follow.
    pub(crate) fn script_end_expecting(expecting: &str) -> SqlError {
        SqlError::new(
            "42000",
            2216,
            format!("Unexpected end of SQL script: Expecting  {expecting}"),
        )
    }

    /// 2220: a cursor that is not open is used. It says where it is only
    /// when `pos` is given.
    pub fn closed_cursor(name: &str, pos: Option<Pos>) -> SqlError {
        let at = pos.map(at).unwrap_or_default();
        SqlError::new(
            "42000",
            2220,
            format!("Error using a closed cursor: {name}{at}"),
        )
    }

    /// 2221: `OPEN` of a cursor that is open.
    pub fn cursor_open(name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2221,
            format!("Cursor is already opened. {name}{}", at(pos)),
        )
    }

    /// 2223: a column of a cursor that is before its first row or after its
    /// last.
    pub fn cursor_no_row(name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2223,
            format!(
                "The cursor is before first row or after last row. Referencing: {name}{}",
                at(pos)
            ),
        )
    }

    /// 2219: `OPEN` of a cursor that was declared without a definition.
    pub fn cursor_undefined(pos: Pos) -> SqlError {
        SqlError::script_error("Cursor is not defined.", pos)
    }

    /// 2234: a parameter in the definition of a cursor.
    pub fn cursor_parameter(pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2234,
            format!(
                "The use of :Param or ? is not allowed in this context: <Cursor \
                 Definition>{} There was a problem parsing the WHERE clause.",
                at(pos)
            ),
        )
    }

    /// 2225: an assignment to a variable that is read only.
    pub(crate) fn read_only(name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2225,
            format!(
                "Invalid assignment in the SQL script. {name} : variable is read only.{}",
                at(pos)
            ),
        )
    }

    /// The ODBC 2 state the original engine puts before the text of an error
    /// in `__errtext`.
    pub fn odbc2_state(&self) -> &str {
        match self.state.as_str() {
            "42000" => "37000",
            "22012" => "S1096",
            "HY000" if !self.message.starts_with("[ASA]") => "S1000",
            state => state,
        }
    }

    /// 2219: a script that cannot run, `why`, at `pos`.
    pub(crate) fn script_error(why: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2219,
            format!("There is an error in the SQL script: {why}{}", at(pos)),
        )
    }

    /// 2115: `BEGIN` is not followed by `TRANSACTION`, `pos` the word after.
    pub(crate) fn begin_without_transaction(pos: Pos) -> SqlError {
        SqlError::expected_explained(
            "",
            " You are missing the keyword TRANSACTION or TRAN after BEGIN.",
            pos,
        )
    }

    /// The error of the statement `text`, with the line and column of its
    /// location added where that is after the first line, as the original
    /// engine does (`fixtures/sql/scripts.json`).
    pub fn with_lines(mut self, text: &str) -> SqlError {
        let marker = "Location of error in the SQL statement is: ";
        let Some(start) = self.message.rfind(marker).map(|i| i + marker.len()) else {
            return self;
        };
        let digits = self.message[start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .count();
        let Ok(pos) = self.message[start..start + digits].parse::<usize>() else {
            return self;
        };
        let before: Vec<char> = text.chars().take(pos.saturating_sub(1)).collect();
        // A line break inside a text is no line break to the original: it
        // is one character or two, and starts no line.
        let mut quoted = false;
        let breaks: Vec<bool> = before
            .iter()
            .map(|&c| {
                if c == '\'' {
                    quoted = !quoted;
                }
                !quoted && c != '\''
            })
            .collect();
        let at_break = |i: usize, c: char| breaks[i] && before[i] == c;
        // The original counts a CR LF as one character in the location
        // (an LF CR as two, a lone CR as one).
        let crlf = (1..before.len())
            .filter(|&i| at_break(i - 1, '\r') && at_break(i, '\n'))
            .count();
        let shown = (pos - crlf).to_string();
        self.message.replace_range(start..start + digits, &shown);
        let digits = shown.len();
        let newlines: Vec<usize> = (0..before.len()).filter(|&i| at_break(i, '\n')).collect();
        let line = 1 + newlines.len();
        if line == 1 {
            return self;
        }
        let column = before.len() - newlines.last().map_or(0, |i| i + 1) + 1;
        self.message
            .insert_str(start + digits, &format!(" (line: {line} column: {column})"));
        self
    }

    /// Takes off where a script said the error was, if it did; whether it
    /// did.
    pub fn strip_script_location(&mut self) -> bool {
        let found = self.message.find(SCRIPT_ERROR);
        if let Some(i) = found {
            self.message.truncate(i);
        }
        found.is_some()
    }

    /// The error of the text `EXECUTE IMMEDIATE` ran, as the original engine
    /// words it for the statement whose text is at `pos`: where it was in
    /// that text, and that it came from there. A script's own exception
    /// keeps its words and takes the position.
    pub fn in_immediate(mut self, pos: Pos) -> SqlError {
        self.strip_script_location();
        if self.native == 2224 {
            self.message.push_str(&script_at(pos));
            return self;
        }
        if self
            .message
            .contains(" -- Location of error in the SQL statement is:")
        {
            self.message =
                self.message
                    .replacen("Location of error in the SQL statement is:", "", 1);
            self.message.push_str(" - EXECUTE IMMEDIATE ");
        } else if self.message.starts_with("[ASA]") {
            self.message.push_str("  - EXECUTE IMMEDIATE ");
        } else {
            self.message.push_str(" - EXECUTE IMMEDIATE ");
        }
        self.message.push_str(&at(pos));
        self
    }

    /// 2115: nothing after `SHOW PLAN FOR`.
    pub(crate) fn no_statement() -> SqlError {
        SqlError::new(
            "42000",
            2115,
            "Expected lexical element not found: Your statement must start with CREATE, DROP, \
             SELECT, INSERT, UPDATE, ALTER, EXECUTE, DELETE, GRANT, REVOKE, BEGIN, COMMIT, \
             ROLLBACK, SAVEPOINT, SET, DEBUG or a script statement."
                .to_owned(),
        )
    }

    /// 2113: the statement ends inside a string or comment.
    pub(crate) fn unexpected_end() -> SqlError {
        SqlError::new("42000", 2113, "Unexpected end of SQL statement".to_owned())
    }

    /// 2115: `CACHE` without `PREPARE` after it, which the original words
    /// without a location.
    pub(crate) fn cache_without_prepare() -> SqlError {
        SqlError::new(
            "42000",
            2115,
            "Expected lexical element not found: PREPARE".to_owned(),
        )
    }

    /// 2117: a token after the end of the statement.
    pub(crate) fn unexpected_token(token: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2117,
            format!(
                "Unexpected token: {token} -- Expecting semicolon.{}",
                at(pos)
            ),
        )
    }

    /// 2117: a token the statement cannot have there, `why`.
    pub(crate) fn unexpected(why: &str, pos: Pos) -> SqlError {
        SqlError::new("42000", 2117, format!("Unexpected token: {why}{}", at(pos)))
    }

    /// 2125: `DROP TABLE` or `ALTER TABLE` (`statement`) of the table `name`
    /// of a link (`link.table`), placed at `pos`.
    pub(crate) fn across_link(name: &str, statement: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2125,
            format!(
                "Invalid table name: {name} - {statement} is not supported across link. {}",
                at(pos)
            ),
        )
    }

    /// 2125: a `SELECT` in `USING` without an alias, placed at `pos`.
    pub(crate) fn missing_alias(pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2125,
            format!(
                "Invalid table name:  Missing alias for subselect in the FROM clause.{}",
                at(pos)
            ),
        )
    }

    /// 2117: a word after `DEBUG` that is no command of the debugger.
    pub(crate) fn unknown_debug_command(token: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2117,
            format!(
                "Unexpected token: {token} -- Expecting BEGIN, END, DATABASE, CONNECTION, RUN, \
                 WAIT, BREAK or REMOVE keyword after the DEBUG keyword.{}",
                at(pos)
            ),
        )
    }

    /// 2233: a command of the SQL debugger on a connection that is not
    /// debugging (`DEBUG END` without `DEBUG BEGIN`).
    pub fn not_debugging(pos: Pos) -> SqlError {
        SqlError::new(
            "HY000",
            2233,
            format!(
                "Illegal debug command: The user is not running as a debugger.{}",
                at(pos)
            ),
        )
    }

    /// 7140: `DEBUG BEGIN` on a connection that is debugging.
    pub fn already_debugging() -> SqlError {
        SqlError::new(
            "HY000",
            7140,
            "[ASA] Error 7140:  The client is already a debugger or debuggee.".to_owned(),
        )
    }

    /// 2200: a command of the SQL debugger tflip does not have.
    pub fn debug_unsupported(command: &str, pos: Pos) -> SqlError {
        SqlError::unsupported(&format!("DEBUG {command}"), pos)
    }

    /// 2215: `CREATE TABLE` options the original engine refuses.
    pub fn invalid_create(why: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "HY000",
            2215,
            format!("Invalid create options: {why}{}", at(pos)),
        )
    }

    /// 2216: a script that ends before its `END`.
    pub(crate) fn script_end(pos: Pos) -> SqlError {
        SqlError::script_end_expecting("END;").located_at(pos)
    }

    /// 2215 without a location: `CREATE INDEX` options that cannot go
    /// together, a `CREATE DATABASE` without its dictionary password.
    pub fn invalid_create_bare(why: &str) -> SqlError {
        SqlError::new("HY000", 2215, format!("Invalid create options: {why}"))
    }

    /// 2116: the page size of `CREATE INDEX` is no whole number; `text` is
    /// what is there instead.
    pub(crate) fn malformed_page_size(text: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2116,
            format!(
                "Malformed number: {text}  Expected positive integer pagesize.{}",
                at(pos)
            ),
        )
    }

    /// 2116: a size in the type of a variable that is no whole number, `found`
    /// what is there instead, `first` whether it is the first of the sizes.
    pub(crate) fn malformed_number(found: &str, first: bool, pos: Pos) -> SqlError {
        let tail = match first {
            true => "expected integer parameter.",
            false => "missing integer in parameter list.",
        };
        SqlError::new(
            "42000",
            2116,
            format!("Malformed number: {found} {tail}{}", at(pos)),
        )
    }

    /// 2116: a size of a column of `CREATE TABLE` or `ALTER TABLE` is no
    /// whole number; `found` is what is there instead, `first` whether it is
    /// the first of the sizes.
    pub(crate) fn malformed_column_size(found: &str, first: bool, pos: Pos) -> SqlError {
        let tail = match first {
            true => "expected positive integer parameter in create",
            false => "missing integer in create parameter list",
        };
        SqlError::new(
            "42000",
            2116,
            format!("Malformed number: {found} {tail}{}", at(pos)),
        )
    }

    /// 5179: a number with decimals assigned to an integer variable.
    pub(crate) fn too_large_bare() -> SqlError {
        SqlError::ace_bare(5179, "The result value exceeded the given buffer size.")
    }

    /// 2124: a logical assigned to a number, or a number to a logical
    /// variable `name`, the value at `pos`; `target` and `source` the names of
    /// the types.
    pub(crate) fn bool_assignment(target: &str, source: &str, name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2124,
            format!(
                "Invalid operand for operator: <assignment> Boolean value is not assignment \
                 compatible with non-Boolean value.{} Target Data Type: <{target}> Source Data \
                 Type: <{source}> Target Name: {name}",
                at(pos)
            ),
        )
    }

    /// 2208: an argument longer than the input parameter it goes to.
    pub fn input_too_large() -> SqlError {
        SqlError::new(
            "HY000",
            2208,
            "An input value was larger than the input parameter.  Check the stored procedure's \
             input parameter definition."
                .to_owned(),
        )
    }

    /// 5179 as a procedure's input parameter has it: a number with decimals
    /// for an integer.
    pub fn input_not_whole() -> SqlError {
        SqlError::ace_bare(5179, "The result value exceeded the given buffer size. ")
    }

    /// 2227: a value out of the range of the variable `name` it is assigned
    /// to (the script's location follows).
    pub(crate) fn variable_out_of_range(name: &str) -> SqlError {
        SqlError::new(
            "42000",
            2227,
            format!("Value assigned to variable is out of range: {name}"),
        )
    }

    /// 2102: a value cut to fit the variable `name`, worded as the original
    /// does, with no space before the name (the script's location follows).
    pub(crate) fn variable_truncated(name: &str) -> SqlError {
        SqlError::new("01004", 2102, format!("Data truncated{name}"))
    }

    /// 2210: an option for a full-text index in a `CREATE INDEX` without
    /// `CONTENT`.
    pub fn full_text_option() -> SqlError {
        SqlError::new(
            "S0000",
            2210,
            "An option in a CREATE INDEX statement was not valid. If you specified options for \
             creating a full text search index, verify that the keyword CONTENT is provided."
                .to_owned(),
        )
    }

    /// 2240: `ALTER TABLE` with both `IGNORE` and `RESPECT TRANSACTIONS`.
    pub(crate) fn both_transaction_options(pos: Pos) -> SqlError {
        SqlError::new(
            "HY000",
            2240,
            format!(
                "The option value specified is not valid: Cannot specify both IGNORE and RESPECT \
                 TRANSACTIONS{}",
                at(pos)
            ),
        )
    }

    /// 2225: `SET` of a system variable that is none, or read only.
    pub fn unknown_system_variable(name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2225,
            format!(
                "Invalid assignment in the SQL script. Unrecognized system variable or the \
                 variable is read only: {name}{}",
                at(pos)
            ),
        )
    }

    /// 2227: `SET ::stmt.unicode_sort_key_size` to a value outside 1 to 8, at
    /// `pos` (NULL has none).
    pub fn sort_key_size_range(pos: Option<Pos>) -> SqlError {
        SqlError::new(
            "42000",
            2227,
            format!(
                "Value assigned to variable is out of range: The Unicode sort key size \
                 multiplier must be between 1 and 8.{}",
                pos.map(at).unwrap_or_default()
            ),
        )
    }

    /// 2124: `SET` of a variable to a logical value, the variable at `pos`.
    pub fn bool_set_value(pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2124,
            format!(
                "Invalid operand for operator: <assignment> Boolean value is not assignment \
                 compatible with non-Boolean value.{}",
                at(pos)
            ),
        )
    }

    /// 2124: `SET` of a variable to a value it does not take, and `RAISE`
    /// with a code that is no integer or a message that is no text.
    pub fn invalid_set_value() -> SqlError {
        SqlError::new(
            "S0000",
            2124,
            "Invalid operand for operator: <assignment>".to_owned(),
        )
    }

    /// 2135: a type `CREATE TABLE` does not know.
    pub fn no_such_type(name: &str) -> SqlError {
        SqlError::new("S0000", 2135, format!("No such type: {name}"))
    }

    /// 2136: a type of `CREATE TABLE` with the wrong number of parameters.
    pub fn type_parameters(name: &str) -> SqlError {
        SqlError::new(
            "S0000",
            2136,
            format!("Invalid number of parameters for: {name}"),
        )
    }

    /// 2136: a system procedure called with arguments it does not take, at
    /// its name.
    pub fn procedure_parameters(name: &str, pos: Pos) -> SqlError {
        SqlError::type_parameters(name).located_at(pos)
    }

    /// 2134: a column name longer than the table type's fields' names.
    pub fn invalid_column_name(name: &str) -> SqlError {
        SqlError::new("S0000", 2134, format!("Invalid column name: {name}"))
    }

    /// 2125: a table name longer than a file's.
    pub fn invalid_table_name(name: &str) -> SqlError {
        SqlError::new("S0000", 2125, format!("Invalid table name: {name}"))
    }

    /// 2009: a table's path of more than 255 characters.
    pub fn path_too_long() -> SqlError {
        SqlError::new(
            "S0000",
            2009,
            "[ISAM]ISAM path or file name too long".to_owned(),
        )
    }

    /// 2157: an index of more than 15 columns.
    pub fn too_many_key_columns() -> SqlError {
        SqlError::new("S0000", 2157, "Too many key columns for index".to_owned())
    }

    /// 2156: an order's name a DBF index file cannot hold (longer than 10
    /// characters).
    pub fn invalid_index_name(name: &str) -> SqlError {
        SqlError::new("S0000", 2156, format!("Invalid index name: {name}"))
    }

    /// 2010: a table `CREATE TABLE` finds there already.
    pub fn table_exists() -> SqlError {
        SqlError::new("S0000", 2010, "[ISAM]ISAM table already exists".to_owned())
    }

    /// 2011: a column an order's key cannot be (a memo field).
    pub fn index_expression() -> SqlError {
        SqlError::new(
            "S0000",
            2011,
            "[ISAM]ISAM index expression not valid".to_owned(),
        )
    }

    /// 2121: a column that is not in the table.
    pub fn column_not_found(name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2121,
            format!("Column not found: {name}{}", at(pos)),
        )
    }

    /// 2121: a column of `CREATE TABLE`'s `PRIMARY KEY` that the table does
    /// not have.
    pub fn primary_key_column(name: &str) -> SqlError {
        SqlError::new(
            "S0000",
            2121,
            format!("Column not found: {name}. The PRIMARY KEY contains an undefined column."),
        )
    }

    /// 2205: a column of a key that cannot be one (a memo field).
    pub fn key_column_type(name: &str) -> SqlError {
        SqlError::new(
            "HY000",
            2205,
            format!("A column specified in the key's column list is of an invalid type: {name}"),
        )
    }

    /// 2122: a qualifier that names no table in the statement, and a column
    /// of a script's cursor that has none of that name.
    pub(crate) fn table_not_found(name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2122,
            format!("Table or alias not found: {name}{}", at(pos)),
        )
    }

    /// 2122 for the alias `RECNO` and `DELETED` are given, which is not
    /// located.
    pub(crate) fn alias_not_found(name: &str) -> SqlError {
        SqlError::new("S0000", 2122, format!("Table or alias not found: {name}"))
    }

    /// 2119: the original's "Internal error", which it raises, with no
    /// location, where it cannot make a type it has no name for.
    pub(crate) fn internal_error() -> SqlError {
        SqlError::new("S0000", 2119, "Internal error".to_owned())
    }

    /// 2124: operands an operator does not take.
    pub(crate) fn invalid_operand(operator: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2124,
            format!("Invalid operand for operator: {operator}{}", at(pos)),
        )
    }

    /// 2124: an operand an operator refuses, and why.
    pub(crate) fn invalid_operand_because(operator: &str, why: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2124,
            format!("Invalid operand for operator: {operator} {why}{}", at(pos)),
        )
    }

    /// 2124: text compared with a date, time or timestamp that it is not.
    pub(crate) fn invalid_operand_value(operator: &str, what: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2124,
            format!(
                "Invalid operand for operator: {operator} [Invalid {what}]{}",
                at(pos)
            ),
        )
    }

    /// 2124: a logical value after a unary `+` or `-`.
    pub(crate) fn not_signable(sign: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2124,
            format!(
                "Invalid operand for operator: {sign} Boolean value is not valid for unary \
                 negative or positive operator.{}",
                at(pos)
            ),
        )
    }

    /// 2124: a logical value where another is expected, or the reverse.
    pub(crate) fn not_boolean(operator: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2124,
            format!(
                "Invalid operand for operator: {operator} Boolean value cannot be operated with \
                 non-Boolean value.{}",
                at(pos)
            ),
        )
    }

    /// 2159: a `WHEN` of a searched `CASE` that is not logical.
    pub(crate) fn case_not_boolean(pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2159,
            format!(
                "Invalid argument to scalar function:  - CASE <The WHEN clause(s) of a search \
                 case expression must be Boolean expression(s).>{}",
                at(pos)
            ),
        )
    }

    /// 2124: a value of a type its column does not take.
    pub(crate) fn invalid_assignment(target: &str, source: &str, name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2124,
            format!(
                "Invalid operand for operator: <assignment> Target Data Type: <{target}> Source \
                 Data Type: <{source}> Target Name: {name}{}",
                at(pos)
            ),
        )
    }

    /// 2124: text that is no `what` (DATE, TIMESTAMP, …) for a column of that
    /// type, named at `pos`.
    pub(crate) fn invalid_assignment_value(
        what: &str,
        pos: Pos,
        target: &str,
        source: &str,
        name: &str,
    ) -> SqlError {
        SqlError::new(
            "S0000",
            2124,
            format!(
                "Invalid operand for operator: <assignment> [Invalid {what}]{} Target Data Type: \
                 <{target}> Source Data Type: <{source}> Target Name: {name}",
                at(pos)
            ),
        )
    }

    /// 2129: an `INSERT` with as many values as columns.
    pub fn insert_count() -> SqlError {
        SqlError::new(
            "S0000",
            2129,
            "Unequal number of insert columns and insert values or the incorrect number of \
             stored procedure input parameters"
                .to_owned(),
        )
    }

    /// 2141: the procedure a cursor is defined on has input parameters, and
    /// the call gives none, its name at `pos`.
    pub fn cursor_parameter_missing(pos: Pos) -> SqlError {
        SqlError::new(
            "07002",
            2141,
            format!(
                "Parameter missing  The stored procedure has input parameters.{}",
                at(pos)
            ),
        )
    }

    /// 2129: the call a cursor is defined on gives a number of arguments
    /// that is not the procedure's.
    pub fn cursor_procedure_arguments() -> SqlError {
        SqlError::new(
            "S0000",
            2129,
            "Unequal number of insert columns and insert values or the incorrect number of \
             stored procedure input parameters <keyword>"
                .to_owned(),
        )
    }

    /// 2133: a column an `INSERT` lists twice.
    pub(crate) fn column_twice(name: &str) -> SqlError {
        SqlError::new(
            "S0000",
            2133,
            format!("Column on list multiple times: {name}"),
        )
    }

    /// 2102: text cut to fit its column, the value at `pos`.
    pub fn truncated(column: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "01004",
            2102,
            format!("Data truncated, column: {column}{}", at(pos)),
        )
    }

    /// 2102: a value too long for its column, no column named (as
    /// `GROUP_CONCAT` past its 1024-byte result reports it).
    pub fn data_truncated() -> SqlError {
        SqlError::new("01004", 2102, "Data truncated".to_owned())
    }

    /// 7057: a key not unique in a unique index, `tag` of `table`.
    pub fn not_unique(table: &str, tag: &str) -> SqlError {
        SqlError::ace(
            7057,
            "The record update failed.  The key value produced from this record was not \
             unique, and an index for the current table has the UNIQUE property.",
            &format!("The key value supplied for {table}:{tag} is not unique."),
        )
    }

    /// 2124 for an aggregate function's argument, without a location.
    pub(crate) fn invalid_aggregate(function: &str) -> SqlError {
        SqlError::new(
            "S0000",
            2124,
            format!("Invalid operand for operator: {function}"),
        )
    }

    /// 2137: a column name in more than one table of a join.
    pub(crate) fn ambiguous_column(name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2137,
            format!("Column found in multiple tables: {name}{}", at(pos)),
        )
    }

    /// 2132: two tables of a statement with the same name or alias.
    pub(crate) fn alias_twice(name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2132,
            format!("Table alias used multiple times: {name}{}", at(pos)),
        )
    }

    /// 2206: the table of an `UPDATE`, in its `FROM` more than once with
    /// an alias each.
    pub(crate) fn update_table_twice(name: &str) -> SqlError {
        SqlError::new(
            "HY000",
            2206,
            format!(
                "The table name is used more than once in the FROM clause of the UPDATE \
                 statement: {name}"
            ),
        )
    }

    /// 2149: an aggregate function in `WHERE`, `ON` or `GROUP BY`.
    pub(crate) fn aggregate_not_allowed(context: &str) -> SqlError {
        SqlError::new(
            "S0000",
            2149,
            format!("Aggregate function not allowed in this context: {context}"),
        )
    }

    /// 2202: a constant where the rows' values are wanted.
    pub(crate) fn constant_not_allowed(context: &str) -> SqlError {
        SqlError::new(
            "HY000",
            2202,
            format!("Constant values are not allowed in this context: {context}"),
        )
    }

    /// 2212: an aggregate function in another's argument.
    pub(crate) fn nested_aggregate(function: &str) -> SqlError {
        SqlError::new(
            "HY000",
            2212,
            format!("Aggregate function cannot be nested: {function}"),
        )
    }

    /// 2196: a column outside aggregates that the `GROUP BY` does not have.
    pub(crate) fn not_grouped(name: &str) -> SqlError {
        SqlError::new(
            "HY000",
            2196,
            format!("Column not found in GROUP BY clause: {name} in SELECT or HAVING clause."),
        )
    }

    /// 2145: a column that cannot be sorted on (a memo).
    pub(crate) fn cannot_order(name: &str) -> SqlError {
        SqlError::new(
            "S0000",
            2145,
            format!("Unable to ORDER BY this column: {name}"),
        )
    }

    /// 2145: the `ORDER BY` of a `UNION` in a script's cursor.
    pub fn union_order_in_cursor() -> SqlError {
        SqlError::cannot_order(
            "ORDER BY on UNION statement is currently not supported in script cursor.",
        )
    }

    /// 2194: a column that `DISTINCT` (or `UNION`) cannot compare.
    pub(crate) fn cannot_distinct(name: &str) -> SqlError {
        SqlError::new(
            "S0000",
            2194,
            format!("Unable to perform DISTINCT operation on this column: {name}"),
        )
    }

    /// 2194 for an expression, which the original engine places.
    pub(crate) fn cannot_distinct_expression(pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2194,
            format!(
                "Unable to perform DISTINCT operation on this column: <expression>{}",
                at(pos)
            ),
        )
    }

    /// 2116: a number with more than one point, read up to the second
    /// (`text`); the original puts its context before a location that is the
    /// number's own, as if it began the statement.
    pub(crate) fn malformed_number_at_start(text: &str, context: &str) -> SqlError {
        SqlError::new(
            "42000",
            2116,
            format!("Malformed number: {text}{context}{}", at(0)),
        )
    }

    /// 2110: a script statement the original engine knows by name only
    /// (`FOR`).
    pub(crate) fn driver_not_capable() -> SqlError {
        SqlError::new("IM001", 2110, "Driver not capable".to_owned())
    }

    /// 2191: the text of `PRINT` or `DPRINT` that is none, at `pos`.
    pub(crate) fn print_not_text(pos: Pos) -> SqlError {
        SqlError::new(
            "HY000",
            2191,
            format!(
                "The data type of the expression is not valid in this context:  Expecting \
                 character expression after PRINT or DPRINT.{}",
                at(pos)
            ),
        )
    }

    /// 2226: a derived table on the right of a `RIGHT` or `FULL JOIN`.
    pub(crate) fn subquery_in_outer_join() -> SqlError {
        SqlError::new(
            "IM001",
            2226,
            "Invalid use of a subquery: Subqueries not allowed in a FULL or RIGHT JOIN.".to_owned(),
        )
    }

    /// 2201: ordinal 0 in `ORDER BY` or `GROUP BY`.
    pub(crate) fn ordinal_invalid(n: impl std::fmt::Display) -> SqlError {
        SqlError::new(
            "HY000",
            2201,
            format!("The column ordinal in the ORDER BY or GROUP BY clause is not valid: {n}"),
        )
    }

    /// 2189: `UNION`ed `SELECT`s with different numbers of columns.
    pub(crate) fn union_columns() -> SqlError {
        SqlError::new(
            "HY000",
            2189,
            "Inconsistent number of columns in the result sets participating in the union \
             statement."
                .to_owned(),
        )
    }

    /// 2191: a `UNION`ed column whose type does not mix with the first's.
    pub(crate) fn union_type(pos: Pos) -> SqlError {
        SqlError::new(
            "HY000",
            2191,
            format!(
                "The data type of the expression is not valid in this context: Incompatible \
                 data type in Union statement.{}",
                at(pos)
            ),
        )
    }

    /// 2166: a subquery with more than one column where a value is wanted.
    pub fn subquery_columns() -> SqlError {
        SqlError::new(
            "S0000",
            2166,
            "SELECT sub-query returns more than one column.".to_owned(),
        )
    }

    /// 2167: a subquery with more than one row where a value is wanted.
    pub fn subquery_rows() -> SqlError {
        SqlError::new(
            "S0000",
            2167,
            "SELECT sub-query returned more than one row.".to_owned(),
        )
    }

    /// 2231: a product whose scale no NUMERIC can have.
    pub(crate) fn numeric_scale(pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2231,
            format!(
                "The result of a numeric algebraic expression is out of range: *The scale of the \
                 numeric algebra exceeded the maximum allowed limit.{}",
                at(pos)
            ),
        )
    }

    /// 2191: the branches of a `CASE`, `IIF` or `COALESCE` whose types do not
    /// mix; `at` is the argument or branch number, `None` for `IIF`.
    pub(crate) fn branch_types(function: &str, at: Option<usize>, pos: Pos) -> SqlError {
        let what = "The data type of the expression is not valid in this context:";
        let message = match (function, at) {
            ("COALESCE", Some(n)) => format!(
                "{what} COALESCE: <Argument types are not compatible: Argument {n}>{}",
                self::at(pos)
            ),
            ("CASE", Some(n)) => {
                format!("{what}  - CASE <return expression in not compatible> <location {n}>")
            }
            _ => format!("{what}  - {function} <Incompatible parameter data types>"),
        };
        SqlError::new("HY000", 2191, message)
    }

    /// 2159: a `CASE` whose `WHEN` value is no match for its operand's type.
    pub(crate) fn case_when_type(n: usize) -> SqlError {
        SqlError::new(
            "S0000",
            2159,
            format!(
                "Invalid argument to scalar function:  - CASE <WHEN Expression> <location {n}>"
            ),
        )
    }

    /// 2159: a `CASE` whose results are all NULL.
    pub(crate) fn case_untyped() -> SqlError {
        SqlError::new(
            "S0000",
            2159,
            "Invalid argument to scalar function:  - CASE <Result type cannot be determined.>"
                .to_owned(),
        )
    }

    /// 2159: `COALESCE` of nothing but NULLs.
    pub(crate) fn coalesce_null(pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2159,
            format!(
                "Invalid argument to scalar function: COALESCE <Result type cannot be NULL>{}",
                at(pos)
            ),
        )
    }

    /// 2214: text of two collations joined.
    pub(crate) fn ambiguous_character() -> SqlError {
        SqlError::new(
            "HY000",
            2214,
            "Invalid coercion: Result of expression is an ambiguous character type.".to_owned(),
        )
    }

    /// 2232: a number too large for the column it goes in.
    pub(crate) fn numeric_overflow() -> SqlError {
        SqlError::new(
            "22012",
            2232,
            "Evaluation of numeric expression resulted in overflow error: ".to_owned(),
        )
    }

    /// 2116: the text of a parameter that is no whole number, where one is
    /// wanted.
    pub(crate) fn parameter_not_integer(pos: Pos) -> SqlError {
        SqlError::new(
            "42000",
            2116,
            format!(
                "Malformed number:  string parameter is not a valid decimal integer.{}",
                at(pos)
            ),
        )
    }

    /// 2116: the text of a parameter that is no number with decimals, as the
    /// parameter has it.
    pub(crate) fn parameter_malformed(text: &str) -> SqlError {
        SqlError::new("42000", 2116, format!("Malformed number: {text}"))
    }

    /// 2232: the text of a parameter that is a number with decimals, where
    /// a whole one is wanted.
    pub(crate) fn parameter_not_whole(pos: Pos) -> SqlError {
        SqlError::new(
            "22012",
            2232,
            format!(
                "Evaluation of numeric expression resulted in overflow error:  numeric \
                 parameter value out of range.{}",
                at(pos)
            ),
        )
    }

    /// 2112: the text of a parameter that is no floating-point number.
    pub(crate) fn assignment_error() -> SqlError {
        SqlError::new("22018", 2112, "Assignment error".to_owned())
    }

    /// 2236: the value of a parameter that cannot be a date, time or
    /// timestamp.
    pub(crate) fn parameter_conversion(pos: Pos) -> SqlError {
        SqlError::new(
            "07006",
            2236,
            format!("Conversion error with parameter{}", at(pos)),
        )
    }

    /// 2213: text of different collations compared.
    pub(crate) fn collation_mismatch() -> SqlError {
        SqlError::new(
            "HY000",
            2213,
            "Invalid comparison or operation on strings with different collations.".to_owned(),
        )
    }

    /// 2209: `RECNO()` or `DELETED()` over more than one table.
    pub(crate) fn table_not_unique() -> SqlError {
        SqlError::new(
            "S0000",
            2209,
            "A reference to a table was not unique. It must be qualified with an alias. "
                .to_owned(),
        )
    }

    /// 2214: `COLLATE` of something that is not text.
    pub(crate) fn invalid_coercion() -> SqlError {
        SqlError::new(
            "HY000",
            2214,
            "Invalid coercion: Explicit coercion is only legal if the column is a string."
                .to_owned(),
        )
    }

    /// 2126: an `ORDER BY` ordinal past the select list.
    pub(crate) fn ordinal_too_large() -> SqlError {
        SqlError::new(
            "S0000",
            2126,
            "ORDER BY or GROUP BY ordinal greater than select list count".to_owned(),
        )
    }

    /// 2131: something other than a value in an `IN` list.
    pub(crate) fn in_not_simple() -> SqlError {
        SqlError::new(
            "S0000",
            2131,
            "Value in an IN expression must be a simple value".to_owned(),
        )
    }

    /// 2141: a parameter without a value at execution.
    pub fn parameter_missing() -> SqlError {
        SqlError::new("07002", 2141, "Parameter missing".to_owned())
    }

    /// 2153: division by zero.
    pub(crate) fn division_by_zero() -> SqlError {
        SqlError::new("22012", 2153, "Division by zero".to_owned())
    }

    /// 4120: division by zero in the expression engine that filters a
    /// table's records.
    pub fn filter_division() -> SqlError {
        SqlError::ace(4120, "Division by zero in record filter expression.", "")
    }

    /// 2111: a value outside the range of its type.
    pub(crate) fn out_of_range() -> SqlError {
        SqlError::new("22003", 2111, "Out of range".to_owned())
    }

    /// 2154: a parameter whose type nothing around it decides.
    pub(crate) fn parameter_type(pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2154,
            format!(
                "The data type of a parameter cannot be determined: {}",
                at(pos)
            ),
        )
    }

    /// 2155: a parameter's text longer than 1024 characters, joined to other
    /// text.
    pub(crate) fn concat_overflow(pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2155,
            format!("String concatenation overflow: {}", at(pos)),
        )
    }

    /// 2159 for a parameter as an argument of `COALESCE`, which takes none;
    /// `argument` counts from 1.
    pub(crate) fn parameter_not_allowed(name: &str, argument: usize, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2159,
            format!(
                "Invalid argument to scalar function: {name}: <Parameter not allowed: \
                 Argument {argument}>{}",
                at(pos)
            ),
        )
    }

    /// 2159 for a parameter that is the operand, a condition or a result of
    /// a `CASE`, which takes none; `pair` counts the `WHEN … THEN` pair it
    /// is in from 1 (the operand and the `ELSE` are in none).
    pub(crate) fn case_parameter(pair: Option<usize>) -> SqlError {
        let location = pair.map_or(String::new(), |n| format!(" <location {n}>"));
        SqlError::new(
            "S0000",
            2159,
            format!(
                "Invalid argument to scalar function:  - CASE <Parameter not allowed>{location}"
            ),
        )
    }

    /// 2159 for bytes as an argument of `name`, which takes none.
    /// `argument` counts from 1.
    pub(crate) fn blob_not_allowed(name: &str, argument: usize, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2159,
            format!(
                "Invalid argument to scalar function: {name}: <Blob arguments not allowed: \
                 Argument {argument}>{}",
                at(pos)
            ),
        )
    }

    /// 2159 for a parameter as a result of `IIF`, which takes none.
    pub(crate) fn iif_parameter(pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2159,
            format!(
                "Invalid argument to scalar function:  - IIF <Parameter not allowed>{}",
                at(pos)
            ),
        )
    }

    /// 2123: a NULL argument whose type nothing decides.
    pub(crate) fn null_type() -> SqlError {
        SqlError::new(
            "S0000",
            2123,
            "Data type of parameter cannot be determined".to_owned(),
        )
    }

    /// 2158: a scalar function the engine does not have.
    pub(crate) fn no_function(name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2158,
            format!("Scalar function not found: {name}{}", at(pos)),
        )
    }

    /// A full-text (`CONTAINS`/`SCORE`) error, by its native engine code: 7094
    /// an unterminated phrase, 7102 no full-text field for `CONTAINS(*, …)`,
    /// 7103 a condition with no searchable word, and the rest the parser raises.
    pub(crate) fn fts(code: u32) -> SqlError {
        SqlError::new(
            "HY000",
            code,
            format!("[ASA] {}", crate::fts::FtsError(code).text()),
        )
    }

    /// 2136: a call of the user-defined function `name` with too many or too
    /// few arguments.
    pub(crate) fn function_arguments(name: &str, too_many: bool, pos: Pos) -> SqlError {
        let what = match too_many {
            true => "Too many actual parameters",
            false => "Not enough actual parameter",
        };
        SqlError::new(
            "S0000",
            2136,
            format!(
                "Invalid number of parameters for: {what} to the function: {name}{}",
                at(pos)
            ),
        )
    }

    /// 2159: an argument the `n`th parameter of the user-defined function
    /// `name` does not take.
    pub(crate) fn function_argument(name: &str, n: usize, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2159,
            format!(
                "Invalid argument to scalar function: Parameter {n} in UDF {name}{}",
                at(pos)
            ),
        )
    }

    /// 2159: arguments a scalar function does not take.
    pub(crate) fn bad_argument(name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2159,
            format!("Invalid argument to scalar function: {name}{}", at(pos)),
        )
    }

    /// 2159 with what is wrong with the arguments after the function's name
    /// and the statement's context after the place.
    pub(crate) fn bad_argument_why(name: &str, why: &str, pos: Pos, trailer: &str) -> SqlError {
        SqlError::new(
            "S0000",
            2159,
            format!(
                "Invalid argument to scalar function: {name}{why}{}{trailer}",
                at(pos)
            ),
        )
    }

    /// 2159 for a `TTOC` format other than 1, 2 or 3.
    pub(crate) fn bad_format(name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2159,
            format!(
                "Invalid argument to scalar function: {name}. The valid format values are 1, 2, \
                 or 3.{}",
                at(pos)
            ),
        )
    }

    /// 2198: a procedure the engine does not have.
    pub fn no_procedure(name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "HY000",
            2198,
            format!("The stored procedure name is invalid: {name}{}", at(pos)),
        )
    }

    /// 2129 as a procedure called with the wrong number of arguments.
    pub fn procedure_arguments() -> SqlError {
        SqlError::insert_count()
    }

    /// 2125: an `EXECUTE PROCEDURE` in `FROM` without an alias, its `)`
    /// at `pos`.
    pub(crate) fn procedure_alias(pos: Pos) -> SqlError {
        SqlError::new(
            "S0000",
            2125,
            format!(
                "Invalid table name:  Missing alias for EXECUTE PROCEDURE in the FROM clause. \
                 There was a problem parsing the table names after the FROM keyword.{}",
                at(pos)
            ),
        )
    }

    /// 2235: a procedure with no output parameters, or a varying one, in
    /// `FROM`.
    pub fn not_selectable(name: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "07002",
            2235,
            format!(
                "Cannot SELECT from a procedure with no output or varying output parameters. \
                 Stored procedure: {name}{}",
                at(pos)
            ),
        )
    }

    /// 2236: text that does not convert to the type it is cast to.
    pub(crate) fn conversion(what: &str, scalar: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "07006",
            2236,
            format!(
                "Conversion error - cannot convert {what}. Scalar: {scalar}{}",
                at(pos)
            ),
        )
    }

    /// 2236 for text that is no date, time or timestamp, which the original
    /// engine words without saying what it was cast to.
    pub(crate) fn cast_failed(scalar: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "07006",
            2236,
            format!("Conversion error with scalar {scalar}{}", at(pos)),
        )
    }

    /// 2236 for binary data too short for the integer it is cast to.
    pub(crate) fn not_enough_binary(scalar: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "07006",
            2236,
            format!(
                "Conversion error not enough binary data to convert into the specified \
                 integer. Scalar: {scalar}{}",
                at(pos)
            ),
        )
    }

    /// 2109: a `DATE`, `TIME` or `TIMESTAMP` literal that is none.
    pub(crate) fn bad_literal() -> SqlError {
        SqlError::new("07006", 2109, "Conversion error".to_owned())
    }

    /// 2118: an escape (`{d '…'}`) that is none, as the text `raw` shows it.
    pub(crate) fn malformed_escape(raw: &str, pos: Pos, trailer: &str) -> SqlError {
        SqlError::new(
            "42000",
            2118,
            format!("Malformed escape clause: {raw}{}{trailer}", at(pos)),
        )
    }

    /// 7200's native code for what tflip's SQL does not do yet.
    pub fn unsupported(what: &str, pos: Pos) -> SqlError {
        SqlError::new(
            "HY000",
            2200,
            format!("tflip does not support {what} yet{}", at(pos)),
        )
    }

    /// 2103: a statement the SQL engine does not support, as the original words
    /// it (a database-wide permission with `ON object`).
    pub(crate) fn not_supported() -> SqlError {
        SqlError::new("IM001", 2103, "Not supported".to_owned())
    }

    /// 2114: a name or number of more than 1024 characters, which the
    /// message gives whole, with no place.
    pub(crate) fn lexical_element_too_large(token: &str) -> SqlError {
        SqlError::new("42000", 2114, format!("Lexical element too large: {token}"))
    }

    /// 2200: `SHOW PLAN FOR` a statement tflip shows no plan of.
    pub fn unsupported_plan() -> SqlError {
        SqlError::unsupported("SHOW PLAN of this statement", 0)
    }

    /// An ACE error underneath the statement, as the original engine wraps
    /// it: `[ASA] Error 5004:  <description> <detail>`.
    pub fn ace(code: u32, description: &str, detail: &str) -> SqlError {
        let text = match detail.is_empty() {
            true => format!("[ASA] Error {code}:  {description}"),
            false => format!("[ASA] Error {code}:  {description}  {detail}"),
        };
        SqlError::new("HY000", code, text)
    }

    /// The ACE error `code` as the SQL engine words some of them, without
    /// the code (7129: `[ASA] The savepoint was not found.`).
    pub fn ace_bare(code: u32, text: &str) -> SqlError {
        SqlError::new("HY000", code, format!("[ASA] {text}"))
    }

    /// 5125: a `system.*` table on a connection that is not to a data
    /// dictionary.
    pub fn no_dictionary() -> SqlError {
        SqlError::new(
            "HY000",
            5125,
            "[ASA] Error 5125:  Advantage does not have a data dictionary associated with the \
             specified handle."
                .to_owned(),
        )
    }

    /// 5095: a date before year 1 or after year 9999.
    pub(crate) fn date_to_julian() -> SqlError {
        SqlError::ace(
            5095,
            "The date specified is not valid.",
            "Date won't convert to julian.",
        )
    }

    /// 5101: a floating point result that is no number.
    pub(crate) fn invalid_float(what: &str, column: &str) -> SqlError {
        SqlError::ace(
            5101,
            "A value was outside of the expected range.",
            &format!("Invalid floating point value ({what}). Field: \"{column}\""),
        )
    }

    /// 5179: a MONEY result beyond the range of the type.
    pub(crate) fn money_too_large(column: &str) -> SqlError {
        SqlError::ace(
            5179,
            "The result value exceeded the given buffer size.",
            &format!("The value is out of the range of the money field type. Field: \"{column}\""),
        )
    }

    /// 5179: a value too large for its column.
    pub(crate) fn too_large(column: &str) -> SqlError {
        SqlError::ace(
            5179,
            "The result value exceeded the given buffer size.",
            &format!("Field: \"{column}\""),
        )
    }
}

/// The mark on the state of an error of a statement that a system procedure
/// ran inside itself.
const INNER: char = '*';

impl SqlError {
    /// This error as the system procedures that run a statement of their own
    /// report it (`fixtures/sql/spddvprops.json`): its state is `HY000`, and
    /// there are two spaces more before the engine's name.
    pub fn inner(mut self) -> SqlError {
        self.state = format!("HY000{INNER}");
        self
    }
}

impl fmt::Display for SqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (state, pad) = match self.state.strip_suffix(INNER) {
            Some(state) => (state, "  "),
            None => (self.state.as_str(), ""),
        };
        write!(
            f,
            "AQE Error:  State = {state};   NativeError = {};  {pad}[SAP][Advantage SQL Engine]{}",
            self.native, self.message
        )
    }
}

impl std::error::Error for SqlError {}

#[cfg(test)]
mod tests {
    use super::*;

    // Measured on the original: a statement of three lines joined by CR LF
    // has its error at 37, as with bare LFs.
    #[test]
    fn crlf_is_one_character_in_the_location() {
        let text = "SELECT a FROM t\r\nWHERE a = 1 AND\r\nzzz zzz";
        let message = "Unexpected token: zzz -- Location of error in the SQL statement is: 39";
        let error = SqlError::new("42000", 2117, message.to_owned()).with_lines(text);
        assert!(error.message.ends_with("is: 37 (line: 3 column: 5)"));
    }
}
