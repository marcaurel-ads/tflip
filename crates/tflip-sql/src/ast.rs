//! Statements as parsed, before names and types are resolved.

use crate::error::Pos;

/// A statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Select(Box<Select>),
    /// `EXECUTE PROCEDURE name(args)`.
    Execute(Execute),
    /// `DEBUG BEGIN`, `DEBUG END`, ...: the SQL debugger's commands; the
    /// position is the command's word.
    Debug(Debug, Pos),
    /// `CACHE PREPARE ON | OFF | DEFAULT`: whether the connection keeps its
    /// prepared statements, which nothing observes.
    CachePrepare(CachePrepare),
    /// `SHOW PLAN FOR statement`: the statement, and its text.
    ShowPlan(Box<Statement>, String),
    /// `INSERT INTO table [(columns)] VALUES (…)[, (…)] | SELECT …`.
    Insert(Box<Insert>),
    /// `UPDATE table SET column = value, … [WHERE …]`.
    Update(Box<Update>),
    /// `DELETE FROM table [WHERE …]`.
    Delete(Box<Delete>),
    /// `MERGE [INTO] table [USING source] ON … WHEN [NOT] MATCHED THEN …`.
    Merge(Box<Merge>),
    /// `CREATE TABLE name (column type [rules], …, [PRIMARY KEY (…)])
    /// [IN DATABASE | AS FREE TABLE] [MEMOBLOCKSIZE n]`.
    CreateTable(Box<CreateTable>),
    /// `CREATE [ONLINE] [UNIQUE] INDEX name ON table (column [DESC], …)
    /// [options]`.
    CreateIndex(Box<CreateIndex>),
    /// `DROP TABLE name [FROM DATABASE [NO_DELETE]]`.
    DropTable(Box<DropTable>),
    /// `DROP INDEX table.name`.
    DropIndex(TableRef, Named),
    /// `ALTER TABLE name action …`.
    AlterTable(Box<AlterTable>),
    /// `CREATE TRIGGER name ON table timing event BEGIN … END [options]`.
    CreateTrigger(Box<CreateTrigger>),
    /// `DROP TRIGGER [table.]name`: the table, if one is named, and the
    /// trigger.
    DropTrigger(Option<Named>, Named),
    /// `CREATE VIEW name AS SELECT …`.
    CreateView(Box<CreateView>),
    /// `DROP VIEW name`.
    DropView(Named),
    /// `CREATE DATABASE`.
    CreateDatabase(Box<CreateDatabase>),
    /// `CREATE PROCEDURE` and `ALTER PROCEDURE`.
    CreateProcedure(Box<CreateProcedure>),
    /// `DROP PROCEDURE name`.
    DropProcedure(Named),
    /// `CREATE`, `ALTER` and `DROP FUNCTION`, `CREATE` and `DROP PACKAGE`.
    FunctionDdl(Box<FunctionDdl>),
    /// `BEGIN TRANSACTION`, `COMMIT`, `ROLLBACK`, `SAVEPOINT`, `SET
    /// TRANSACTION`.
    Transaction(Transaction),
    /// `GRANT …` or `REVOKE …`.
    Grant(Box<Grant>),
    /// A text sent as a statement that is a script: more than one
    /// statement, or one of the script language's own.
    Script(Vec<ScriptStmt>),
    /// `SET ::conn.Collation = expr` and the other system variables.
    SetVariable(Box<SetVariable>),
}

/// `DROP TABLE`.
#[derive(Debug, Clone, PartialEq)]
pub struct DropTable {
    pub table: TableRef,
    /// `FROM DATABASE`: the table must be one of the dictionary's.
    pub from_database: bool,
    /// `NO_DELETE`: the table's files stay.
    pub no_delete: bool,
}

/// `SET ::scope.member = value`.
#[derive(Debug, Clone, PartialEq)]
pub struct SetVariable {
    /// The scope before the dot (`conn`, `stmt`), in lower case.
    pub scope: String,
    /// The variable after it, as written, and where.
    pub member: String,
    pub member_pos: Pos,
    pub value: Expr,
}

/// `GRANT permission, … [ON object] TO grantee [WITH GRANT]` or `REVOKE [GRANT
/// OPTION FOR] permission, … [ON object] FROM grantee`.
#[derive(Debug, Clone, PartialEq)]
pub struct Grant {
    pub revoke: bool,
    /// `WITH GRANT` on a grant, `GRANT OPTION FOR` on a revoke: the grant option.
    pub grant_option: bool,
    /// The permissions, in the order written (the engine applies them so).
    pub permissions: Vec<Permission>,
    /// The `ON` object, by its bare name.
    pub object: Option<String>,
    pub grantee: String,
}

/// One permission of a `GRANT` or `REVOKE`.
#[derive(Debug, Clone, PartialEq)]
pub enum Permission {
    /// A right on the `ON` object (an `ADS_PERMISSION_*` bit) or, for `SELECT`,
    /// `INSERT` and `UPDATE`, on one of its columns.
    Object { right: u32, column: Option<String> },
    /// `ALL`: every right the object allows.
    All,
    /// `ALTER DATABASE` or `INHERIT DATABASE`: the right on the database itself.
    Database(u32),
    /// `[INHERIT] CREATE kind`: the right to create objects of the
    /// `ADS_DD_*_OBJECT` type `object_type`, or with `inherit` its inherit bit.
    Create { object_type: u16, inherit: bool },
}

impl Permission {
    /// Whether this permission is about the `ON` object (not database-wide).
    pub fn needs_object(&self) -> bool {
        matches!(self, Permission::Object { .. } | Permission::All)
    }
}

/// A statement in a SQL script — a stored procedure or trigger body: the
/// control-flow and variable statements of the script language, and the ordinary
/// SQL statements it runs.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptStmt {
    /// `DECLARE name type;` — a local variable (`ty` is the type as written).
    Declare { name: String, ty: String },
    /// `name = expr;` or `SET name = expr;` (`set`) — an assignment to a
    /// variable; `pos` is where `name` is.
    Assign {
        name: String,
        value: Expr,
        pos: Pos,
        set: bool,
    },
    /// `IF cond THEN ... [ELSE ...] END IF;`
    If {
        cond: Expr,
        then: Vec<ScriptStmt>,
        els: Vec<ScriptStmt>,
    },
    /// `WHILE cond DO ... END WHILE;`
    While { cond: Expr, body: Vec<ScriptStmt> },
    /// `BEGIN ... END;` — a block.
    Block(Vec<ScriptStmt>),
    /// `RETURN [expr];`
    Return(Option<Expr>),
    /// `LEAVE;` in a `WHILE`: ends the innermost loop.
    Leave,
    /// `CONTINUE;` in a `WHILE`: goes on with its next pass.
    Continue,
    /// `RAISE name(code, message);` — fails the script with its own
    /// exception; `pos` is where `name` is, which the error reports.
    Raise {
        name: String,
        code: Expr,
        message: Expr,
        pos: usize,
    },
    /// `EXECUTE IMMEDIATE expr;` — runs the text `expr` gives as a script of
    /// its own; `pos` is where `expr` is.
    ExecuteImmediate { expr: Expr, pos: Pos },
    /// `EXECUTE PRINT expr;` or `EXECUTE DPRINT expr;`: text for the debugger,
    /// which a script without one only checks; `pos` is where `expr` is.
    Print { expr: Expr, pos: Pos },
    /// `DECLARE name CURSOR [AS SELECT … | AS EXECUTE PROCEDURE …];` — a
    /// cursor, with its definition if it has one (`OPEN` may give it); `pos`
    /// is where `name` is.
    DeclareCursor {
        name: String,
        def: Option<Box<Statement>>,
        pos: Pos,
    },
    /// `OPEN name [AS SELECT … | AS EXECUTE PROCEDURE …];`
    OpenCursor {
        name: String,
        def: Option<Box<Statement>>,
        pos: Pos,
    },
    /// `FETCH name;`
    FetchCursor { name: String, pos: Pos },
    /// `CLOSE name;`
    CloseCursor { name: String, pos: Pos },
    /// `IF [NOT] FETCH name THEN … [ELSE …] END IF;`
    IfFetch {
        name: String,
        negated: bool,
        pos: Pos,
        then: Vec<ScriptStmt>,
        els: Vec<ScriptStmt>,
    },
    /// `WHILE [NOT] FETCH name DO … END WHILE;`
    WhileFetch {
        name: String,
        negated: bool,
        pos: Pos,
        body: Vec<ScriptStmt>,
    },
    /// `RAISE;` in a `CATCH`: passes the error being handled on; `pos` is its `;`.
    Reraise { pos: Pos },
    /// `TRY … CATCH … FINALLY … END TRY;`
    Try {
        body: Vec<ScriptStmt>,
        catches: Vec<Catch>,
        finally: Option<Vec<ScriptStmt>>,
    },
    /// An ordinary SQL statement run as part of the script, where it starts,
    /// and its text as written (from its first token to its last, without
    /// the `;`), which the query log keeps.
    Sql(Box<Statement>, Pos, String),
}

/// A `CATCH` block of a `TRY`.
#[derive(Debug, Clone, PartialEq)]
pub struct Catch {
    /// The class of exception it handles: the name of a `RAISE`, or
    /// `ADS_SCRIPT_EXCEPTION` for the engine's own errors; `None` for `ALL`.
    pub class: Option<String>,
    pub body: Vec<ScriptStmt>,
}

/// A statement on the connection's transaction.
#[derive(Debug, Clone, PartialEq)]
pub enum Transaction {
    /// `BEGIN TRAN[SACTION]`.
    Begin,
    /// `COMMIT [WORK]`.
    Commit,
    /// `ROLLBACK [WORK] [TO [SAVEPOINT] name]`.
    Rollback(Option<String>),
    /// `SAVEPOINT name`.
    Savepoint(String),
    /// `SET TRAN[SACTION] AUTOCOMMIT_ON | AUTOCOMMIT_OFF | EXPLICIT`, upper
    /// case.
    Set(String),
}

/// `ALTER TABLE`.
#[derive(Debug, Clone, PartialEq)]
pub struct AlterTable {
    pub table: TableRef,
    pub actions: Vec<AlterAction>,
    /// Where the first column constraint is, which a free table cannot have.
    pub constraint: Option<Pos>,
    /// `ONLINE` or `ONLINE_WITH_TRANSITION`: the table stays open for other
    /// users, which only a remote server does.
    pub online: bool,
    /// `ONLINE_WITH_TRANSITION`, which waits for the connections that opened
    /// the table meanwhile to disconnect.
    pub transition: bool,
}

/// What `ALTER TABLE` does.
#[derive(Debug, Clone, PartialEq)]
pub enum AlterAction {
    /// `ADD [COLUMN] column [POSITION n]`.
    Add(ColumnDef, Option<u64>),
    /// `ALTER [COLUMN] old column [POSITION n]`.
    Alter(String, ColumnDef, Option<u64>),
    /// `DROP [COLUMN] name`.
    Drop(String),
    /// `MEMOBLOCKSIZE n`.
    MemoBlockSize(u64),
    /// `IGNORE TRANSACTIONS` (true) or `RESPECT TRANSACTIONS`.
    Transactions(bool),
    /// `DROP [CONSTRAINT] PRIMARY KEY`.
    DropPrimaryKey,
    /// `ALTER [COLUMN] column DROP rule`, the rule and where the column is.
    DropRule(String, Pos, ColumnRule),
}

/// What `ALTER COLUMN … DROP` takes away from a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnRule {
    NotNull,
    Minimum,
    Maximum,
    ErrorMessage,
    Default,
}

/// `CREATE VIEW`.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateView {
    pub name: Named,
    /// The `SELECT` as the dictionary keeps it: from its first word to its
    /// last token, without the comments and `;` around it.
    pub text: String,
    pub select: Select,
}

/// `CREATE DATABASE name [DESCRIPTION d] [PASSWORD p] [ENCRYPT TRUE | FALSE]
/// [ENCRYPTIONTYPE RC4 | AES128 | AES256] [DDPASSWORD p]`: a new data
/// dictionary, as far as tflip writes one.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateDatabase {
    /// The dictionary's file, as written.
    pub name: String,
    pub description: String,
    /// `ENCRYPTIONTYPE AES128 | AES256`: the encryption type of its tables
    /// (5 or 6), with the `DDPASSWORD` their key is made of; `None` for one
    /// of the default encryption.
    pub aes: Option<(u8, String)>,
    /// `ENCRYPT TRUE`: the dictionary encrypts its records, with AES if it has
    /// a type, else with RC4.
    pub encrypt: bool,
}

/// `CREATE PROCEDURE name (params) BEGIN script END | FUNCTION f IN [COM]LIBRARY
/// lib`, or `ALTER PROCEDURE`, which takes the same.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateProcedure {
    pub alter: bool,
    pub name: Named,
    /// The parameters but `CURSOR VARYING OUTPUT`, in the order written.
    pub params: Vec<ProcParam>,
    /// Whether `CURSOR VARYING OUTPUT` is among them.
    pub varying: bool,
    pub body: ProcBody,
}

/// A statement on the functions and packages of a dictionary.
#[derive(Debug, Clone, PartialEq)]
pub enum FunctionDdl {
    /// `CREATE FUNCTION` and `ALTER FUNCTION`.
    Create(CreateFunction),
    /// `DROP FUNCTION [package.]name`.
    Drop { package: Option<Named>, name: Named },
    /// `CREATE PACKAGE name [DESCRIPTION text]`.
    CreatePackage {
        name: Named,
        description: Option<String>,
    },
    /// `DROP PACKAGE name`.
    DropPackage(Named),
}

/// `CREATE FUNCTION [package.]name (params) RETURNS type [DESCRIPTION text]
/// BEGIN script END`, or `ALTER FUNCTION [package.]name [new name] (...)`.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateFunction {
    pub alter: bool,
    pub package: Option<Named>,
    pub name: Named,
    /// `ALTER FUNCTION`'s new name, which may repeat the package's.
    pub new_name: Option<Named>,
    /// The parameters as written, between the parentheses and trimmed.
    pub params: String,
    /// How many there are.
    pub param_count: usize,
    /// The return type as the dictionary spells it (`CHAR ( 20 )`).
    pub returns: String,
    pub description: Option<String>,
    /// The script between `BEGIN` and `END`, as a procedure keeps it.
    pub body: String,
}

/// A parameter of a procedure: an input, or an `OUTPUT` one.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcParam {
    pub column: ColumnDef,
    pub output: bool,
}

/// What a procedure runs.
#[derive(Debug, Clone, PartialEq)]
pub enum ProcBody {
    /// The script between `BEGIN` and `END` as written, but for the line
    /// break after `BEGIN`.
    Script(String),
    /// A function of a library.
    External {
        function: String,
        library: String,
        /// `COMLIBRARY`: a COM or .NET class, not a file.
        com: bool,
    },
}

/// `CREATE TRIGGER`: a table trigger, or a database trigger (`ON DATABASE`),
/// with a SQL script body, as the dictionary keeps it
/// (`AdsDDCreateTrigger`'s arguments).
#[derive(Debug, Clone, PartialEq)]
pub struct CreateTrigger {
    pub name: Named,
    /// The table, or `DATABASE` for a database trigger.
    pub table: Named,
    /// When it runs (`ADS_TRIGTYPE_*`): 1 `BEFORE`, 2 `INSTEAD OF`, 4
    /// `AFTER`, 8 `CONFLICT`.
    pub timing: u32,
    /// What it runs on (`ADS_TRIGEVENT_*`): 1 `INSERT`, 2 `UPDATE`, 3
    /// `DELETE`; of a database trigger 4 `OPEN_TABLE`, 5 `CLOSE_TABLE`, 6
    /// `CONNECT`, 7 `DISCONNECT`.
    pub event: u32,
    /// The script between `BEGIN` and `END`, as written; empty for a
    /// trigger that is a function of a library.
    pub body: String,
    /// `FUNCTION f IN LIBRARY name`, the container that is not a script.
    pub library: Option<TriggerLibrary>,
    /// `ADS_TRIGOPTIONS_*`: 1 the values (unless `NO VALUES`), 2 the memo
    /// and BLOB values (unless `NO MEMOS`), 4 `NO TRANSACTION`.
    pub options: u32,
    /// `PRIORITY n`, 1 without.
    pub priority: u32,
    /// `ALTER TRIGGER`, which takes the same and rewrites the trigger.
    pub alter: bool,
}

/// The function of a library a trigger runs: `FUNCTION function IN LIBRARY
/// library` (a file), or `IN ASSEMBLY` (a COM or .NET class).
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerLibrary {
    pub function: String,
    pub library: String,
    pub assembly: bool,
}

/// `CREATE TABLE`.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateTable {
    pub table: TableRef,
    pub columns: Vec<ColumnDef>,
    /// `MEMOBLOCKSIZE n`.
    pub block_size: Option<u64>,
    /// The `PRIMARY KEY`, a column's or the table's, if there is one.
    pub primary_key: Option<PrimaryKey>,
    /// Where `IN DATABASE` is, if it is there.
    pub in_database: Option<Pos>,
    /// `AS FREE TABLE`: a free table, on a dictionary connection too.
    pub free: bool,
    /// `IGNORE TRANSACTIONS`: the table is transaction-free.
    pub ignore_transactions: bool,
}

/// The `PRIMARY KEY` of `CREATE TABLE`: the name `CONSTRAINT name` gives
/// it, and its columns, each descending or not.
#[derive(Debug, Clone, PartialEq)]
pub struct PrimaryKey {
    pub name: Option<String>,
    pub columns: Vec<(Named, bool)>,
}

/// A column of `CREATE TABLE`: its name, its type as written and where,
/// the numbers in parentheses after the type, whether it is `COMPRESSED`,
/// and the rules a dictionary keeps for it.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    pub name_pos: Pos,
    pub type_name: String,
    pub type_pos: Pos,
    pub params: Vec<u64>,
    pub compressed: bool,
    /// A plain `NULL` (`Some(true)`) or `NOT NULL`, which only a Visual
    /// FoxPro table takes.
    pub nullable: Option<bool>,
    /// `NOCPTRANS`: no translation between code pages, which only a Visual
    /// FoxPro table takes.
    pub nocptrans: bool,
    pub rules: FieldRules,
}

/// What `CREATE TABLE` says of a column beyond its type: `CONSTRAINT NOT
/// NULL` (a plain `NOT NULL` is [`ColumnDef::nullable`]), `DEFAULT`, `CONSTRAINT
/// MINIMUM`, `CONSTRAINT MAXIMUM` and `CONSTRAINT ERROR MESSAGE`, each
/// value as written.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldRules {
    pub not_null: bool,
    pub default: Option<String>,
    pub minimum: Option<String>,
    pub maximum: Option<String>,
    pub message: Option<String>,
}

impl FieldRules {
    /// Whether there is any rule.
    pub fn any(&self) -> bool {
        *self != FieldRules::default()
    }
}

/// `CREATE INDEX`.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateIndex {
    pub unique: bool,
    /// `ONLINE`: the table stays open for other users while the index is
    /// built, which only a remote server does.
    pub online: bool,
    /// `CANDIDATE`: a true unique index on a VFP table.
    pub candidate: bool,
    /// `NOTDELETED`: the index leaves out the records marked deleted.
    pub not_deleted: bool,
    /// `IN FILE name`: the index file of a dictionary table.
    pub file: Option<String>,
    /// `PAGESIZE n`.
    pub page_size: Option<u64>,
    /// Where `CONTENT` is: a full-text index.
    pub content: Option<Pos>,
    /// The options of a full-text index.
    pub full_text: FullText,
    pub name: Named,
    pub table: TableRef,
    /// The columns of the key, and whether each is descending.
    pub columns: Vec<(Named, bool)>,
}

/// The options of a full-text index (`CREATE INDEX … CONTENT`): its word
/// bounds, flags, and character and noise lists, each with whether `NEW`
/// replaces the defaults with it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FullText {
    pub min_word: Option<u64>,
    pub max_word: Option<u64>,
    pub delimiters: Option<(bool, String)>,
    pub noise: Option<(bool, String)>,
    pub drop: Option<(bool, String)>,
    pub conditionals: Option<(bool, String)>,
    /// `NOTMAINTAINED`.
    pub fixed: bool,
    pub case_sensitive: bool,
    pub keep_score: bool,
    pub protect_numbers: bool,
}

/// A column a statement names, and where.
#[derive(Debug, Clone, PartialEq)]
pub struct Named {
    pub name: String,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Insert {
    pub table: TableRef,
    /// The columns given values; all of them, in order, if `None`.
    pub columns: Option<Vec<Named>>,
    pub source: InsertSource,
}

/// The rows an `INSERT` adds.
#[derive(Debug, Clone, PartialEq)]
pub enum InsertSource {
    Values(Vec<Vec<Expr>>),
    Select(Box<Select>),
}

/// `UPDATE table [WITH DELETE | WITH RECALL] SET column = value, …
/// [FROM tables] [WHERE …]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    /// The table changed: with a `FROM`, the table it joins that the name
    /// after `UPDATE` names.
    pub table: TableRef,
    /// The name after `UPDATE`, which `SET`'s qualifiers must match.
    pub named: String,
    /// What the records written are marked as.
    pub mark: Mark,
    /// Each column set, and its new value.
    pub sets: Vec<MergeSet>,
    /// The tables whose rows give the values and the condition.
    pub from: Option<From>,
    pub filter: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Delete {
    pub table: TableRef,
    pub filter: Option<Expr>,
}

/// `MERGE`: for each row of the source, the target's rows the condition
/// holds for are updated, or a row is inserted if there are none.
#[derive(Debug, Clone, PartialEq)]
pub struct Merge {
    /// The target, with its alias.
    pub table: TableRef,
    /// `USING`: a table or a `SELECT` in parentheses; without one the
    /// statement runs once.
    pub source: Option<From>,
    pub on: Expr,
    pub update: Option<MergeUpdate>,
    pub insert: Option<MergeInsert>,
}

/// What a DBF record is marked as by the statement that writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Keep,
    /// `WITH DELETE`.
    Delete,
    /// `WITH RECALL`.
    Recall,
}

/// `WHEN MATCHED THEN UPDATE [WITH DELETE | WITH RECALL] SET …`.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeUpdate {
    pub mark: Mark,
    pub sets: Vec<MergeSet>,
}

/// A column `MERGE` or `UPDATE` sets, which may be qualified with the
/// target's name.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeSet {
    pub qualifier: Option<String>,
    /// The column, where the qualifier or the column starts.
    pub column: Named,
    pub value: Expr,
}

/// `WHEN NOT MATCHED THEN INSERT [WITH DELETE] …`.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeInsert {
    /// Whether the record inserted is a deleted one.
    pub deleted: bool,
    /// The columns given values; all of them if `None`.
    pub columns: Option<Vec<Named>>,
    pub source: InsertSource,
}

impl Statement {
    /// Calls `f` on every table the statement names.
    pub fn for_each_table<'a>(&'a self, f: &mut dyn FnMut(&'a TableRef)) {
        let exprs = |e: &'a Expr, f: &mut dyn FnMut(&'a TableRef)| {
            e.walk(&mut |e| match &e.kind {
                ExprKind::Subquery(s) | ExprKind::Exists(s) => s.for_each_table(f),
                ExprKind::InSelect { select, .. } | ExprKind::Quantified { select, .. } => {
                    select.for_each_table(f)
                }
                _ => {}
            })
        };
        match self {
            Statement::Select(select) => select.for_each_table(f),
            Statement::Insert(insert) => {
                f(&insert.table);
                match &insert.source {
                    InsertSource::Values(rows) => {
                        rows.iter().flatten().for_each(|e| exprs(e, f));
                    }
                    InsertSource::Select(select) => select.for_each_table(f),
                }
            }
            Statement::Update(update) => {
                f(&update.table);
                update.sets.iter().for_each(|set| exprs(&set.value, f));
                if let Some(from) = &update.from {
                    from.for_each_source(&mut |source| {
                        if let SourceRef::Table(table) = source {
                            f(table);
                        }
                    });
                }
                update.filter.iter().for_each(|e| exprs(e, f));
            }
            Statement::Delete(delete) => {
                f(&delete.table);
                delete.filter.iter().for_each(|e| exprs(e, f));
            }
            Statement::Merge(merge) => {
                f(&merge.table);
                match &merge.source {
                    Some(From::Table(table)) => f(table),
                    Some(From::Derived { select, .. }) => select.for_each_table(f),
                    _ => {}
                }
                exprs(&merge.on, f);
                if let Some(update) = &merge.update {
                    update.sets.iter().for_each(|set| exprs(&set.value, f));
                }
                if let Some(insert) = &merge.insert {
                    match &insert.source {
                        InsertSource::Values(rows) => {
                            rows.iter().flatten().for_each(|e| exprs(e, f));
                        }
                        InsertSource::Select(select) => select.for_each_table(f),
                    }
                }
            }
            Statement::ShowPlan(inner, _) => inner.for_each_table(f),
            Statement::CreateTable(create) => f(&create.table),
            Statement::CreateIndex(create) => f(&create.table),
            Statement::DropTable(drop) => f(&drop.table),
            Statement::DropIndex(table, _) => f(table),
            Statement::AlterTable(alter) => f(&alter.table),
            Statement::Execute(_)
            | Statement::Debug(..)
            | Statement::CachePrepare(_)
            | Statement::Transaction(_)
            | Statement::Grant(_)
            | Statement::SetVariable(_)
            | Statement::CreateTrigger(_)
            | Statement::DropTrigger(..)
            | Statement::CreateView(_)
            | Statement::DropView(_)
            | Statement::CreateDatabase(_)
            | Statement::CreateProcedure(_)
            | Statement::DropProcedure(_)
            | Statement::FunctionDdl(_) => {}
            // Its statements open their tables as they run.
            Statement::Script(_) => {}
        }
    }

    /// Calls `f` on every procedure's result the statement reads in a
    /// `FROM`, by its number and call.
    pub fn for_each_procedure<'a>(&'a self, f: &mut dyn FnMut(usize, &'a Execute)) {
        fn select<'a>(select: &'a Select, f: &mut dyn FnMut(usize, &'a Execute)) {
            select.for_each_source(&mut |source| {
                if let SourceRef::Procedure { id, execute } = source {
                    f(id, execute);
                }
            });
        }
        fn expr<'a>(expr: &'a Expr, f: &mut dyn FnMut(usize, &'a Execute)) {
            expr.walk(&mut |e| match &e.kind {
                ExprKind::Subquery(s) | ExprKind::Exists(s) => select(s, f),
                ExprKind::InSelect { select: s, .. } | ExprKind::Quantified { select: s, .. } => {
                    select(s, f)
                }
                _ => {}
            });
        }
        match self {
            Statement::Select(s) => select(s, f),
            Statement::Insert(insert) => match &insert.source {
                InsertSource::Values(rows) => rows.iter().flatten().for_each(|e| expr(e, f)),
                InsertSource::Select(s) => select(s, f),
            },
            Statement::Update(update) => {
                update.sets.iter().for_each(|set| expr(&set.value, f));
                update.filter.iter().for_each(|e| expr(e, f));
            }
            Statement::Delete(delete) => delete.filter.iter().for_each(|e| expr(e, f)),
            Statement::ShowPlan(inner, _) => inner.for_each_procedure(f),
            _ => {}
        }
    }
}

/// What `CACHE PREPARE` is set to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePrepare {
    On,
    Off,
    Default,
}

/// A command of the SQL debugger that takes no arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Debug {
    /// Makes the connection a debugger.
    Begin,
    /// Ends the connection's debugging.
    End,
    Wait,
    Database,
}

/// `SELECT …`.
#[derive(Debug, Clone, PartialEq)]
pub struct Select {
    /// `{static}`: a static cursor whatever the statement.
    pub force_static: bool,
    pub top: Option<Top>,
    pub distinct: bool,
    pub items: Vec<Item>,
    /// `INTO table`: the free table the result is stored in.
    pub into: Option<Named>,
    pub from: From,
    pub filter: Option<Expr>,
    /// `GROUP BY`: select list ordinals or expressions.
    pub group: Vec<OrderKey>,
    pub having: Option<Expr>,
    /// The `SELECT`s after `UNION` (with `ALL` or not), which the `ORDER BY`
    /// sorts together with this one.
    pub unions: Vec<(bool, Select)>,
    pub order: Vec<OrderItem>,
}

impl Select {
    /// The table a `SELECT` of one table reads.
    pub fn table(&self) -> Option<&TableRef> {
        match &self.from {
            From::Table(table) => Some(table),
            _ => None,
        }
    }

    /// Whether the `SELECT` reads one table without grouping, aggregates,
    /// subqueries or `UNION`: what a live cursor can show.
    pub fn is_simple(&self) -> bool {
        matches!(self.from, From::Table(_)) && self.is_simple_over_one_source()
    }

    /// [`Self::is_simple`] for a `SELECT` of a `SELECT` in parentheses,
    /// which is a live cursor too where that one reads one table.
    pub fn is_simple_over_derived(&self) -> bool {
        matches!(self.from, From::Derived { .. }) && self.is_simple_over_one_source()
    }

    fn is_simple_over_one_source(&self) -> bool {
        let complex = |e: &Expr| {
            e.any(&|e| {
                matches!(
                    e.kind,
                    ExprKind::Aggregate { .. }
                        | ExprKind::Subquery(_)
                        | ExprKind::Exists(_)
                        | ExprKind::InSelect { .. }
                        | ExprKind::Quantified { .. }
                )
            })
        };
        let items = self.items.iter().any(|item| match item {
            Item::Expr { expr, .. } => complex(expr),
            Item::Star { .. } => false,
        });
        let order = self
            .order
            .iter()
            .any(|o| matches!(&o.key, OrderKey::Expr(e) if complex(e)));
        self.group.is_empty()
            && self.having.is_none()
            && self.unions.is_empty()
            && !items
            && !order
            && !self.filter.as_ref().is_some_and(complex)
    }

    /// Calls `f` on every table the statement names, its subqueries' and
    /// `UNION`s' included.
    pub fn for_each_table<'a>(&'a self, f: &mut dyn FnMut(&'a TableRef)) {
        self.for_each_source(&mut |source| {
            if let SourceRef::Table(table) = source {
                f(table);
            }
        });
    }

    /// Calls `f` on every table and procedure the statement reads, its
    /// subqueries' and `UNION`s' included.
    pub fn for_each_source<'a>(&'a self, f: &mut dyn FnMut(SourceRef<'a>)) {
        fn exprs<'a>(expr: &'a Expr, f: &mut dyn FnMut(SourceRef<'a>)) {
            expr.walk(&mut |e| match &e.kind {
                ExprKind::Subquery(s) | ExprKind::Exists(s) => s.for_each_source(f),
                ExprKind::InSelect { select, .. } | ExprKind::Quantified { select, .. } => {
                    select.for_each_source(f)
                }
                _ => {}
            });
        }
        self.from.for_each_source(f);
        for item in &self.items {
            if let Item::Expr { expr, .. } = item {
                exprs(expr, f);
            }
        }
        for e in self.filter.iter().chain(&self.having) {
            exprs(e, f);
        }
        for key in self.group.iter().chain(self.order.iter().map(|o| &o.key)) {
            if let OrderKey::Expr(e) = key {
                exprs(e, f);
            }
        }
        for (_, select) in &self.unions {
            select.for_each_source(f);
        }
    }
}

/// A table or a procedure's result a statement reads.
#[derive(Debug, Clone, Copy)]
pub enum SourceRef<'a> {
    Table(&'a TableRef),
    Procedure { id: usize, execute: &'a Execute },
}

/// What a `SELECT` reads: a table, a `SELECT` in parentheses, or a join.
#[derive(Debug, Clone, PartialEq)]
pub enum From {
    Table(TableRef),
    /// `(SELECT …) alias`.
    Derived {
        select: Box<Select>,
        alias: Option<String>,
        pos: Pos,
    },
    /// `(EXECUTE PROCEDURE name(args)) alias`: the procedure's result.
    Procedure {
        execute: Execute,
        /// The procedure's number among the statement's tables and
        /// procedures.
        id: usize,
        alias: Option<String>,
        /// The `)` after the call.
        pos: Pos,
    },
    /// `a [INNER|LEFT|RIGHT|FULL] JOIN b ON …`, or `a, b` (an inner join
    /// without a condition).
    Join {
        kind: JoinKind,
        left: Box<From>,
        right: Box<From>,
        on: Option<Expr>,
    },
}

impl From {
    /// Calls `f` on every table and procedure the `FROM` reads, its
    /// subqueries' included.
    pub fn for_each_source<'a>(&'a self, f: &mut dyn FnMut(SourceRef<'a>)) {
        fn exprs<'a>(expr: &'a Expr, f: &mut dyn FnMut(SourceRef<'a>)) {
            expr.walk(&mut |e| match &e.kind {
                ExprKind::Subquery(s) | ExprKind::Exists(s) => s.for_each_source(f),
                ExprKind::InSelect { select, .. } | ExprKind::Quantified { select, .. } => {
                    select.for_each_source(f)
                }
                _ => {}
            });
        }
        match self {
            From::Table(table) => f(SourceRef::Table(table)),
            From::Procedure { execute, id, .. } => {
                f(SourceRef::Procedure { id: *id, execute });
                for arg in &execute.args {
                    exprs(arg, f);
                }
            }
            From::Derived { select, .. } => select.for_each_source(f),
            From::Join {
                left, right, on, ..
            } => {
                left.for_each_source(f);
                right.for_each_source(f);
                if let Some(on) = on {
                    exprs(on, f);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    Inner,
    Left,
    Right,
    Full,
}

/// `TOP n [START AT m]`, or `TOP n PERCENT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Top {
    pub count: u64,
    /// The first row, 1-based.
    pub start: u64,
    /// `count` is a percentage of the rows, rounded up.
    pub percent: bool,
}

/// An item of the select list.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// `*`, or `t.*`.
    Star {
        table: Option<String>,
        pos: Pos,
    },
    Expr {
        expr: Expr,
        alias: Option<String>,
    },
}

/// The table a `SELECT` reads.
#[derive(Debug, Clone, PartialEq)]
pub struct TableRef {
    /// The table's name (`system.<name>` for a system table).
    pub name: String,
    /// The link to another data dictionary the table is reached through
    /// (`link.table`), or the path of such a dictionary, as written.
    pub link: Option<String>,
    pub alias: Option<String>,
    pub pos: Pos,
    /// Where the alias is, if there is one.
    pub alias_pos: Pos,
    /// The table's number in the statement, in the order tables appear.
    pub id: usize,
}

/// An `ORDER BY` key.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderItem {
    pub key: OrderKey,
    pub descending: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OrderKey {
    /// A column of the select list, 1-based.
    Ordinal(u64, Pos),
    Expr(Expr),
}

/// `EXECUTE PROCEDURE name(args)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Execute {
    pub name: String,
    /// Where `EXECUTE` is.
    pub keyword: Pos,
    pub pos: Pos,
    pub args: Vec<Expr>,
}

/// An expression and where it starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// A number, as written.
    Number(String),
    Str(String),
    /// `x'2A2B'`: the bytes.
    Bin(Vec<u8>),
    Bool(bool),
    Null,
    /// `DEFAULT` in the values of an `INSERT`: the column is left as a new
    /// record has it.
    Default,
    /// `{d '…'}`, `{t '…'}`, `{ts '…'}`: the letter and the text.
    Typed(TypedLiteral, String),
    /// A column, maybe qualified by a table name or alias.
    Column {
        table: Option<String>,
        name: String,
    },
    /// A parameter: its number (0-based, in order of appearance).
    Param(usize),
    /// `expr COLLATE ads_default_ci | ads_default_cs`: whether it is the
    /// case-insensitive one.
    Collate {
        expr: Box<Expr>,
        ci: bool,
    },
    Neg(Box<Expr>),
    /// A unary `+`, which the original engine checks as it does `-`.
    Plus(Box<Expr>),
    /// `~`: bitwise not.
    BitNot(Box<Expr>),
    Not(Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },
    Between {
        expr: Box<Expr>,
        low: Box<Expr>,
        high: Box<Expr>,
        negated: bool,
    },
    In {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    Like {
        expr: Box<Expr>,
        pattern: Box<Expr>,
        escape: Option<Box<Expr>>,
        negated: bool,
    },
    /// `CASE [operand] WHEN … THEN … [ELSE …] END`.
    Case {
        operand: Option<Box<Expr>>,
        whens: Vec<(Expr, Expr)>,
        otherwise: Option<Box<Expr>>,
    },
    /// A scalar function: its name as written, and its arguments.
    Func {
        name: String,
        args: Vec<Expr>,
    },
    /// `CONTAINS(field, condition)`: whether the full-text condition matches the
    /// field. `all` is `CONTAINS(*, …)`, over every full-text field, with no
    /// `field`.
    Contains {
        all: bool,
        field: Option<Box<Expr>>,
        condition: Box<Expr>,
    },
    /// `SCORE`/`SCOREDISTINCT`: `SCORE(field, condition)` (or `SCORE(*, …)`)
    /// scores a full-text match; `SCORE(n)` is the score of the `n`th `CONTAINS`
    /// of the `WHERE`.
    Score {
        distinct: bool,
        spec: ScoreSpec,
    },
    /// `CAST(e AS type)` and `CONVERT(e, type)`.
    Cast {
        expr: Box<Expr>,
        to: CastType,
        function: &'static str,
    },
    /// `EXTRACT(part FROM e)`.
    Extract {
        part: String,
        expr: Box<Expr>,
    },
    /// `TRIM([LEADING|TRAILING|BOTH] [chars] FROM e)`.
    Trim {
        side: TrimSide,
        chars: Option<Box<Expr>>,
        expr: Box<Expr>,
    },
    /// `POSITION(a IN b)`.
    Position(Box<Expr>, Box<Expr>),
    /// `COUNT(*)` (no argument), `COUNT([DISTINCT] e)`, `SUM(e)`, ...,
    /// `GROUP_CONCAT([DISTINCT] e [ORDER BY …] [SEPARATOR 'str'])`.
    Aggregate {
        func: Aggregate,
        arg: Option<Box<Expr>>,
        distinct: bool,
        /// `GROUP_CONCAT` only: the separator; `None` is the default comma.
        separator: Option<String>,
        /// `GROUP_CONCAT` only: its inner `ORDER BY` (empty is record order);
        /// the keys are expressions, never select-list ordinals.
        order: Vec<OrderItem>,
    },
    /// `(SELECT …)` as a value.
    Subquery(Box<Select>),
    /// `EXISTS (SELECT …)`.
    Exists(Box<Select>),
    /// `e [NOT] IN (SELECT …)`.
    InSelect {
        expr: Box<Expr>,
        select: Box<Select>,
        negated: bool,
    },
    /// `e op ALL (SELECT …)`, `e op ANY (SELECT …)`.
    Quantified {
        op: BinOp,
        expr: Box<Expr>,
        select: Box<Select>,
        all: bool,
    },
}

/// What a `SCORE`/`SCOREDISTINCT` scores.
#[derive(Debug, Clone, PartialEq)]
pub enum ScoreSpec {
    /// `SCORE(field, condition)`, or `SCORE(*, condition)` (`all`).
    Field {
        all: bool,
        field: Option<Box<Expr>>,
        condition: Box<Expr>,
    },
    /// `SCORE(n)`: the score of the `n`th `CONTAINS` of the `WHERE` (1-based).
    Ordinal(i64),
}

/// An aggregate function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggregate {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    /// `GROUP_CONCAT`: the group's text values joined by a separator. Its
    /// separator and inner `ORDER BY` are on the [`ExprKind::Aggregate`].
    GroupConcat,
}

impl Aggregate {
    pub fn name(self) -> &'static str {
        match self {
            Aggregate::Count => "COUNT",
            Aggregate::Sum => "SUM",
            Aggregate::Avg => "AVG",
            Aggregate::Min => "MIN",
            Aggregate::Max => "MAX",
            Aggregate::GroupConcat => "GROUP_CONCAT",
        }
    }
}

impl Expr {
    /// Calls `f` on the expression and every expression in it, but not
    /// into subqueries.
    pub fn walk<'a>(&'a self, f: &mut dyn FnMut(&'a Expr)) {
        f(self);
        self.for_each_child(&mut |e| e.walk(f));
    }

    /// Calls `f` on each expression directly in this one (its operands and
    /// arguments), but not into subqueries.
    pub fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a Expr)) {
        let mut each = |e: &'a Expr| f(e);
        match &self.kind {
            ExprKind::Number(_)
            | ExprKind::Str(_)
            | ExprKind::Bin(_)
            | ExprKind::Bool(_)
            | ExprKind::Null
            | ExprKind::Default
            | ExprKind::Typed(..)
            | ExprKind::Column { .. }
            | ExprKind::Param(_)
            | ExprKind::Subquery(_)
            | ExprKind::Exists(_) => {}
            ExprKind::Neg(a) | ExprKind::Plus(a) | ExprKind::BitNot(a) | ExprKind::Not(a) => {
                each(a)
            }
            ExprKind::Binary(_, a, b) | ExprKind::Position(a, b) => {
                each(a);
                each(b);
            }
            ExprKind::IsNull { expr, .. }
            | ExprKind::Collate { expr, .. }
            | ExprKind::Cast { expr, .. }
            | ExprKind::Extract { expr, .. }
            | ExprKind::InSelect { expr, .. }
            | ExprKind::Quantified { expr, .. } => each(expr),
            ExprKind::Between {
                expr, low, high, ..
            } => {
                each(expr);
                each(low);
                each(high);
            }
            ExprKind::In { expr, list, .. } => {
                each(expr);
                list.iter().for_each(each);
            }
            ExprKind::Like {
                expr,
                pattern,
                escape,
                ..
            } => {
                each(expr);
                each(pattern);
                if let Some(e) = escape {
                    each(e);
                }
            }
            ExprKind::Case {
                operand,
                whens,
                otherwise,
            } => {
                if let Some(e) = operand {
                    each(e);
                }
                for (w, t) in whens {
                    each(w);
                    each(t);
                }
                if let Some(e) = otherwise {
                    each(e);
                }
            }
            ExprKind::Func { args, .. } => args.iter().for_each(each),
            ExprKind::Contains {
                field, condition, ..
            } => {
                if let Some(f) = field {
                    each(f);
                }
                each(condition);
            }
            ExprKind::Score { spec, .. } => {
                if let ScoreSpec::Field {
                    field, condition, ..
                } = spec
                {
                    if let Some(f) = field {
                        each(f);
                    }
                    each(condition);
                }
            }
            ExprKind::Trim { chars, expr, .. } => {
                if let Some(c) = chars {
                    each(c);
                }
                each(expr);
            }
            ExprKind::Aggregate { arg, order, .. } => {
                if let Some(a) = arg {
                    each(a);
                }
                for item in order {
                    if let OrderKey::Expr(e) = &item.key {
                        each(e);
                    }
                }
            }
        }
    }

    /// Whether the expression or one in it (not in subqueries) satisfies
    /// `f`.
    pub fn any(&self, f: &dyn Fn(&Expr) -> bool) -> bool {
        let mut found = false;
        self.walk(&mut |e| found |= f(e));
        found
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedLiteral {
    Date,
    Time,
    TimeStamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimSide {
    Leading,
    Trailing,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    BitOr,
    BitAnd,
    BitXor,
    Shl,
    Shr,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

impl BinOp {
    /// The operator as written, for messages.
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Or => "OR",
            BinOp::And => "AND",
            BinOp::Eq => "=",
            BinOp::Ne => "<>",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::BitOr => "|",
            BinOp::BitAnd => "&",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
        }
    }

    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }
}

/// The type of `CAST` and `CONVERT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastType {
    Char(Option<u16>),
    VarChar(Option<u16>),
    LongVarChar,
    WChar(Option<u16>),
    WVarChar(Option<u16>),
    WLongVarChar,
    Binary(Option<u16>),
    VarBinary(Option<u16>),
    LongVarBinary,
    Bit,
    TinyInt,
    SmallInt,
    Integer,
    BigInt,
    Numeric(Option<(u16, u16)>),
    Decimal(Option<(u16, u16)>),
    Float,
    Double,
    Real,
    Money,
    Date,
    Time,
    TimeStamp,
    Guid,
    CiChar(Option<u16>),
}
