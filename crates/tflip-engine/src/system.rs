//! The `system.*` tables, as a dictionary connection sees them in SQL
//! (`SELECT * FROM system.tables`, ...). Each is a fixed set of columns
//! whose rows are the dictionary's objects and their properties, built in
//! memory and read like any other table.
//!
//! `system.objects` (the catalog of the system tables), `system.fts` (the
//! full-text settings), `system.systemgroups` and `system.systemprocedures`
//! work on any connection; the others need a data
//! dictionary ([`SqlError::no_dictionary`] without one). Columns the
//! dictionary keeps encrypted — passwords — are left `NULL`. User group
//! memberships (`system.usergroupmembers`) and permission grants
//! (`system.permissions`, and `system.effectivepermissions` which rolls a user's
//! groups' grants into the user) are read when the connection's administrator
//! password recovers the dictionary key; without it, memberships fall back to
//! `DB:Public` and grants to zero (see `docs/KNOWN_ISSUES.md`).

use tflip_sql::SqlError;
use tflip_storage::build::Column;
use tflip_storage::crypto::Scheme;
use tflip_storage::dd::{self, Kind, Object};
use tflip_storage::value::Value;

use crate::dictionary::Database;

mod holes;

/// A built system table: its columns and its rows, each a value per column.
pub struct SystemTable {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
}

/// The full-text settings a dictionary has not set, which `system.fts` and
/// `system.dictionary` fall back to (the original engine's defaults).
mod fts_default {
    pub const DELIMITERS: &[u8] = b"\x08\t\n\x0b\x0c\r ";
    pub const NOISE: &str = "about after all also an and another any are as at be because been \
        before being between both but by came can come could did do does each else for from get \
        got had has have he her here him himself his how if in into is it its just like make many \
        me might more most much must my never now of on only or other our out over re said same \
        see should since so some still such take than that the their them then there these they \
        this those through to too under up use very want was way we well were what when where \
        which while who will with would you your";
    pub const DROP_CHARS: &str = "'\"`";
    pub const CONDITIONAL_CHARS: &str = ",.?!;:@#$%^&()-_";
}

/// Whether `name` (without `system.`) is a system table.
pub fn is_system_table(name: &str) -> bool {
    schema(name).is_some()
}

/// Whether the listing `name` (lower case) fails with 5127 on `database`
/// because an object it reads has a `DB:Public` rights slot that does not
/// check.
fn listing_refused(database: &Database, name: &str) -> bool {
    let bad = |kinds: &[Kind]| database.any_public_slot_bad(kinds);
    // A user other than the administrator reads the slots of the objects it
    // lists, whether or not rights are verified; one held to its rights
    // reads those of the tables and views that decide what it sees.
    let ordinary = |kinds: &[Kind]| database.is_ordinary_user() && bad(kinds);
    let held = |kinds: &[Kind]| database.lists_by_rights() && bad(kinds);
    match name {
        "tables" => held(&[Kind::Table]) || ordinary(&[Kind::Database]),
        "columns" | "indexfiles" => held(&[Kind::Table]),
        "triggers" => held(&[Kind::Table, Kind::View]),
        "views" => ordinary(&[Kind::View]),
        "storedprocedures" => ordinary(&[Kind::Procedure]),
        "functions" => ordinary(&[Kind::Function]),
        "users" => ordinary(&[Kind::User]),
        "usergroups" => ordinary(&[Kind::Group]),
        "links" => ordinary(&[Kind::Link]),
        "packages" => ordinary(&[Kind::Package]),
        "publications" | "publicationarticles" | "articles" => ordinary(&[Kind::Publication]),
        "subscriptions" => ordinary(&[Kind::Subscription]),
        "dictionary" => ordinary(&[Kind::Database]),
        _ => false,
    }
}

/// [`build`] for the table `name` that `select` reads: the permissions with
/// the rows a rights slot that does not check takes from the statement
/// (`fixtures/sql/ddpublicbad3`, `ddpublicbad4`, `ddpublicbad5`).
pub fn build_for(
    database: Option<&Database>,
    name: &str,
    select: &tflip_sql::ast::Select,
) -> Result<Option<SystemTable>, SqlError> {
    let Some(mut built) = build(database, name)? else {
        return Ok(None);
    };
    let effective = match name.to_ascii_lowercase().as_str() {
        "permissions" => false,
        "effectivepermissions" => true,
        _ => return Ok(Some(built)),
    };
    let Some(database) = database else {
        return Ok(Some(built));
    };
    let mut bad = permission_bad_rows(database, effective);
    if let Some(keep) = first_table_only(select, &built.rows) {
        let mut flags = keep.iter();
        built.rows.retain(|_| *flags.next().unwrap());
        let mut flags = keep.iter();
        bad.retain(|_| *flags.next().unwrap());
    }
    if bad.contains(&true)
        && let Some(usage) = holes::Usage::of(select, &built.columns)
    {
        built.rows = holes::apply(built.rows, &bad, &usage)?;
    }
    Ok(Some(built))
}

/// The rows a `WHERE Object_Type = 4` keeps of the permissions' grid, if it
/// is the condition: the original lists the columns of the first table
/// only, unless a `Parent = '...'` names another
/// (`fixtures/sql/ddeffperm`, `ddpublicbad9`).
fn first_table_only(select: &tflip_sql::ast::Select, rows: &[Vec<Value>]) -> Option<Vec<bool>> {
    use tflip_sql::ast::{BinOp, ExprKind};
    let mut conjuncts = Vec::new();
    let mut pending: Vec<&tflip_sql::ast::Expr> = select.filter.iter().collect();
    while let Some(expr) = pending.pop() {
        match &expr.kind {
            ExprKind::Binary(BinOp::And, a, b) => pending.extend([a.as_ref(), b.as_ref()]),
            _ => conjuncts.push(expr),
        }
    }
    let equals = |expr: &tflip_sql::ast::Expr, column: &str, literal: fn(&ExprKind) -> bool| {
        let ExprKind::Binary(BinOp::Eq, a, b) = &expr.kind else {
            return false;
        };
        let named = |e: &tflip_sql::ast::Expr| matches!(&e.kind, ExprKind::Column { name, .. } if name.eq_ignore_ascii_case(column));
        (named(a) && literal(&b.kind)) || (named(b) && literal(&a.kind))
    };
    let columns = |kind: &ExprKind| matches!(kind, ExprKind::Number(n) if n == "4");
    let parent = |kind: &ExprKind| matches!(kind, ExprKind::Str(_));
    select.table()?;
    if !conjuncts.iter().any(|e| equals(e, "Object_Type", columns))
        || conjuncts.iter().any(|e| equals(e, "Parent", parent))
    {
        return None;
    }
    let first = rows
        .iter()
        .find(|row| matches!(row.get(1), Some(Value::Integer(4))))
        .and_then(|row| row.get(2));
    Some(
        rows.iter()
            .map(|row| !matches!(row.get(1), Some(Value::Integer(4))) || row.get(2) == first)
            .collect(),
    )
}

/// Builds the system table `name` (the part after `system.`) for a
/// connection whose dictionary is `database` (`None` for a free-table
/// connection). `Ok(None)` if `name` is not a system table; an error if it
/// is one but the connection has no dictionary to build it from.
pub fn build(database: Option<&Database>, name: &str) -> Result<Option<SystemTable>, SqlError> {
    let Some(columns) = schema(name) else {
        return Ok(None);
    };
    let columns = columns_of(&columns);
    // The catalog and the full-text settings do not need a dictionary.
    let name = name.to_ascii_lowercase();
    // An object whose rights slot for DB:Public does not check fails the
    // listings that read it, for a user held to its rights or for any user but
    // the administrator, by the kind of object
    // (`fixtures/sql/ddpublicbad`, `ddpublicbad4`, `ddpublicbad5`).
    if let Some(database) = database
        && listing_refused(database, &name)
    {
        return Err(SqlError::ace(
            5127,
            crate::ace_error::error_text(5127).unwrap_or_default(),
            "Inconsistency detected while reading stored property.",
        ));
    }
    if name == "objects" {
        return Ok(Some(SystemTable {
            rows: objects_rows(),
            columns,
        }));
    }
    if name == "fts" {
        return Ok(Some(SystemTable {
            rows: vec![fts_row(database)],
            columns,
        }));
    }
    if name == "systemgroups" {
        return Ok(Some(SystemTable {
            rows: system_group_rows(),
            columns,
        }));
    }
    if name == "systemprocedures" {
        return Ok(Some(SystemTable {
            rows: system_procedure_rows(),
            columns,
        }));
    }
    let database = database.ok_or_else(SqlError::no_dictionary)?;
    let rows = rows(database, &name);
    Ok(Some(SystemTable { rows, columns }))
}

/// `T` or `F`.
fn logical(on: bool) -> Value {
    Value::Logical(on)
}

/// A whole number, or `NULL` for none.
fn number(value: Option<impl Into<i128>>) -> Value {
    value.map_or(Value::Null, |v| Value::Integer(v.into()))
}

/// Text, or `NULL` for none.
fn text(value: Option<String>) -> Value {
    value.map_or(Value::Null, Value::Text)
}

/// The rows of the dictionary system table `name` (lower case, not
/// `objects` or `fts`).
fn rows(database: &Database, name: &str) -> Vec<Vec<Value>> {
    let dd = database.dictionary();
    match name {
        "dictionary" => vec![dictionary_row(database)],
        "tables" => dd
            .all(Kind::Table)
            .iter()
            .filter(|t| database.shows(t))
            .map(|t| table_row(database, t))
            .collect(),
        "columns" => column_rows(database),
        "indexfiles" => index_file_rows(database),
        "indexes" => index_rows(database),
        "views" => dd
            .all(Kind::View)
            .iter()
            .filter(|v| database.shows(v))
            .map(|v| view_row(database, v))
            .collect(),
        "storedprocedures" => dd
            .all(Kind::Procedure)
            .iter()
            .filter(|p| database.shows(p))
            .map(|p| procedure_row(database, p))
            .collect(),
        "functions" => {
            // Grouped by the package that owns them, the database's first
            // (`fixtures/sql/ddfuncs`).
            let mut functions = dd.all(Kind::Function);
            functions.sort_by_key(|f| (f.parent, f.id));
            functions
                .iter()
                .filter(|f| database.shows(f))
                .map(|f| function_row(database, f))
                .collect()
        }
        "triggers" => {
            // Grouped by the object they are on: the database's first, then
            // each table's, as the original engine lists them.
            let mut triggers = dd.all(Kind::Trigger);
            triggers.sort_by_key(|t| (t.parent, t.record()));
            triggers
                .iter()
                .filter(|t| database.lists_trigger(t))
                .map(|t| trigger_row(database, t))
                .collect()
        }
        "users" => dd.all(Kind::User).iter().map(|u| user_row(u)).collect(),
        "usergroups" => group_rows(database),
        "usergroupmembers" => member_rows(database),
        "relations" => dd
            .all(Kind::Relation)
            .iter()
            .filter(|_| database.sees_everything())
            .map(|r| relation_row(database, r))
            .collect(),
        "links" => dd
            .all(Kind::Link)
            .iter()
            .filter(|l| database.shows(l))
            .map(|l| link_row(database, l))
            .collect(),
        "packages" => dd
            .all(Kind::Package)
            .iter()
            .filter(|p| database.shows(p))
            .map(|p| package_row(p))
            .collect(),
        "permissions" => permission_rows(database),
        "effectivepermissions" => effective_permission_rows(database),
        "publications" => dd
            .all(Kind::Publication)
            .iter()
            .filter(|p| database.shows(p))
            .map(|p| publication_row(p))
            .collect(),
        "publicationarticles" | "articles" => article_rows(database),
        "subscriptions" => dd
            .all(Kind::Subscription)
            .iter()
            .filter(|s| database.shows(s))
            .map(|s| subscription_row(database, s))
            .collect(),
        _ => Vec::new(),
    }
}

/// The columns of `system.dictionary` a user who is not the administrator
/// reads (`fixtures/sql/ddvisible`); the others are `NULL` for it.
const DICTIONARY_PUBLIC: [usize; 13] = [0, 1, 2, 3, 13, 14, 15, 16, 17, 18, 20, 23, 24];

fn dictionary_row(database: &Database) -> Vec<Value> {
    let mut row = dictionary_values(database);
    if !database.sees_everything() {
        for (i, value) in row.iter_mut().enumerate() {
            if !DICTIONARY_PUBLIC.contains(&i) {
                *value = Value::Null;
            }
        }
    }
    row
}

/// The table encryption password, `NULL` if there is none or the connection's
/// password does not recover it.
fn table_password(database: &Database) -> Value {
    let password = database.dictionary().table_password(database.password());
    text(
        password
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(&p).into_owned()),
    )
}

fn dictionary_values(database: &Database) -> Vec<Value> {
    use dd::database::*;
    let db = match database.dictionary().database() {
        Some(db) => db,
        None => return Vec::new(),
    };
    let u16_or = |slot, default: u16| number(Some(db.u16(slot).unwrap_or(default)));
    // The path the dictionary keeps relative to its folder, or `.\`.
    let relative = |slot| Value::Text(db.text(slot).unwrap_or_else(|| ".\\".to_owned()));
    vec![
        u16_or(VERSION_MAJOR, 0),
        u16_or(VERSION_MINOR, 0),
        relative(DEFAULT_TABLE_PATH),
        relative(TEMP_TABLE_PATH),
        logical(!db.has(LOGIN_NOT_REQUIRED)),
        logical(!db.has(RIGHTS_NOT_CHECKED)),
        // The password is kept under the dictionary key, which the
        // connection's login recovers.
        table_password(database),
        logical(db.has(ENCRYPT_NEW_TABLE)),
        logical(db.u16(ENABLE_INTERNET) == Some(1)),
        u16_or(INTERNET_SECURITY_LEVEL, 2),
        u16_or(MAX_FAILED_ATTEMPTS, 5),
        logical(db.u16(LOGINS_DISABLED) == Some(1)),
        text(db.text(LOGINS_DISABLED_MESSAGE)),
        text(db.comment.clone()),
        binary(db.slot(USER_DEFINED)),
        text(db.text(FTS_DELIMITERS)),
        text(db.text(FTS_NOISE)),
        text(db.text(FTS_DROP_CHARS)),
        text(db.text(FTS_CONDITIONAL_CHARS)),
        logical(database.dictionary().encrypted()),
        logical(db.has(ENCRYPT_INDEXES)),
        text(db.text(QUERY_LOG_TABLE)),
        logical(db.has(ENCRYPT_COMMUNICATION)),
        logical(db.has(DISABLE_DLL_CACHING)),
        logical(db.has(TRIGGERS_DISABLED)),
        number(Some(db.u32(DATA_ENCRYPTION_TYPE).unwrap_or(3))),
        number(Some(db.u32(QUERY_VIA_ROOT).unwrap_or(0))),
        logical(db.u16(ENFORCE_MAX_FAILED_LOGINS) == Some(1)),
    ]
}

fn table_row(database: &Database, table: &Object) -> Vec<Value> {
    let mut row = table_values(database, table);
    // How the table is kept is for a user who may alter it, and its caching
    // for the administrator (`fixtures/sql/ddvisible`).
    hide(&mut row, &[1, 3, 7], !database.may_alter(1, &table.name));
    hide(&mut row, &[14], !database.sees_everything());
    row
}

/// Sets the columns `which` of `row` to `NULL` if `hidden`.
fn hide(row: &mut [Value], which: &[usize], hidden: bool) {
    if hidden {
        for &i in which {
            row[i] = Value::Null;
        }
    }
}

fn table_values(database: &Database, table: &Object) -> Vec<Value> {
    use dd::table::*;
    let flags = table.info2.unwrap_or(0);
    let level = match flags {
        f if f & flags::PERMISSION_LEVEL_1 != 0 => 1,
        f if f & flags::PERMISSION_LEVEL_3 != 0 => 3,
        _ => 2,
    };
    let primary_key = database.primary_key(table).map(|index| index.name.clone());
    vec![
        Value::Text(table.name.clone()),
        text(table.text(RELATIVE_PATH)),
        number(Some(crate::dictionary::ads_table_type(table))),
        logical(!table.has(NO_AUTO_CREATE)),
        text(primary_key),
        text(table.text(DEFAULT_INDEX)),
        // Tables are not encrypted yet.
        logical(false),
        number(Some(level)),
        number(table.u16(MEMO_BLOCK_SIZE)),
        text(table.text(VALIDATION_EXPR)),
        text(table.text(VALIDATION_MSG)),
        text(table.comment.clone()),
        binary(table.slot(USER_DEFINED)),
        logical(table.has(TRIGGERS_DISABLED)),
        number(Some(flags >> flags::CACHING_SHIFT & 3)),
        logical(flags & flags::TRANSACTION_FREE != 0),
        logical(flags & flags::WEB_DELTA != 0),
        logical(flags & flags::CONCURRENCY != 0),
    ]
}

fn column_rows(database: &Database) -> Vec<Vec<Value>> {
    use dd::field::*;
    let dd = database.dictionary();
    let mut rows = Vec::new();
    for table in dd.all(Kind::Table).iter().filter(|t| database.shows(t)) {
        for field in dd.children(table.id, Kind::Field) {
            if !database.shows_column(table, field) {
                continue;
            }
            let definition = field.slot(DEFINITION).unwrap_or_default();
            let part = |i: usize| {
                definition
                    .get(2 * i..2 * i + 2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
            };
            let mut row = vec![
                Value::Text(field.name.clone()),
                Value::Text(table.name.clone()),
                number(field.info1),
                number(part(0)),
                number(part(1).map(u32::from)),
                number(part(2)),
                text(field.text(MIN_VALUE)),
                text(field.text(MAX_VALUE)),
                logical(!field.has(NOT_NULL)),
                text(field.text(DEFAULT_VALUE)),
                text(field.text(VALIDATION_MSG)),
                text(field.comment.clone()),
                binary(field.slot(USER_DEFINED)),
                number(field.u32(OPTIONS)),
            ];
            hide(&mut row, &[8], !database.may_write(&table.name));
            rows.push(row);
        }
    }
    rows
}

/// An index file's path as the view shows it: the slot without its last NUL,
/// so that a path a move or a rename left a byte longer ends in one.
fn index_file_path(file: &dd::Object) -> Option<String> {
    let bytes = file.slot(dd::index_file::RELATIVE_PATH)?;
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    Some(tflip_storage::cp1252::decode(bytes))
}

fn index_file_rows(database: &Database) -> Vec<Vec<Value>> {
    use dd::index_file::*;
    let dd = database.dictionary();
    let mut rows = Vec::new();
    for table in dd.all(Kind::Table).iter().filter(|t| database.shows(t)) {
        for file in dd.children(table.id, Kind::IndexFile) {
            rows.push(vec![
                Value::Text(file.name.clone()),
                Value::Text(table.name.clone()),
                text(index_file_path(file)),
                number(file.u32(PAGE_SIZE)),
            ]);
        }
    }
    rows
}

fn index_rows(database: &Database) -> Vec<Vec<Value>> {
    use dd::index::*;
    let dd = database.dictionary();
    let mut rows = Vec::new();
    for table in dd.all(Kind::Table).iter().filter(|t| database.shows(t)) {
        for index in dd.children(table.id, Kind::Index) {
            let options = index.u32(OPTIONS).unwrap_or(0);
            // A unique ADI order is reported a candidate too.
            let options = options | if options & 1 != 0 { 0x800 } else { 0 };
            let mut row = vec![
                Value::Text(index.name.clone()),
                Value::Text(table.name.clone()),
                text(database.name_of(index.info1.map(|i| i as u32))),
                text(index.text(EXPRESSION)),
                text(index.text(CONDITION)),
                number(Some(options)),
                number(index.u16(KEY_LENGTH)),
                number(index.u16(FTS_MIN_LENGTH)),
                text(index.text(FTS_DELIMITERS)),
                text(index.text(FTS_NOISE)),
                text(index.text(FTS_DROP_CHARS)),
                text(index.text(FTS_CONDITIONAL_CHARS)),
                text(index.comment.clone()),
                text(index.text(COLLATION)),
            ];
            hide(
                &mut row,
                &[3, 4, 5, 13],
                !database.may_alter(1, &table.name),
            );
            rows.push(row);
        }
    }
    rows
}

fn view_row(database: &Database, view: &Object) -> Vec<Value> {
    use dd::view::*;
    let mut row = vec![
        Value::Text(view.name.clone()),
        number(view.u16(STATEMENT_LENGTH)),
        text(view.text(STATEMENT)),
        text(view.comment.clone()),
        logical(view.has(TRIGGERS_DISABLED)),
    ];
    hide(&mut row, &[1, 2], !database.may_alter(6, &view.name));
    row
}

fn procedure_row(database: &Database, procedure: &Object) -> Vec<Value> {
    use dd::procedure::*;
    let mut row = vec![
        Value::Text(procedure.name.clone()),
        text(procedure.text(INPUT)),
        text(procedure.text(OUTPUT)),
        text(procedure.text(DLL_NAME)),
        text(procedure.text(DLL_FUNCTION_NAME)),
        text(procedure.comment.clone()),
        number(procedure.u32(INVOKE_OPTION)),
        text(procedure.text(SCRIPT)),
    ];
    hide(&mut row, &[6, 7], !database.may_alter(10, &procedure.name));
    row
}

fn function_row(database: &Database, function: &Object) -> Vec<Value> {
    use dd::function::*;
    let package = database
        .dictionary()
        .object(function.parent)
        .filter(|p| p.kind == Kind::Package)
        .map(|p| p.name.clone());
    let mut row = vec![
        Value::Text(function.name.clone()),
        text(package),
        text(function.text(RETURN_TYPE)),
        text(function.text(PARAMETERS)),
        text(function.text(SCRIPT)),
        text(function.comment.clone()),
        binary(function.slot(dd::field::USER_DEFINED)),
    ];
    hide(&mut row, &[4], !database.may_alter(18, &function.name));
    row
}

fn trigger_row(database: &Database, trigger: &Object) -> Vec<Value> {
    use dd::trigger::*;
    let table = trigger
        .info1
        .and_then(|id| database.dictionary().object(id as u32))
        .filter(|t| t.kind == Kind::Table)
        .map(|t| t.name.clone());
    vec![
        Value::Text(trigger.name.clone()),
        text(table),
        number(trigger.u32(EVENT_TYPE)),
        number(trigger.u32(TRIGGER_TYPE)),
        number(trigger.u32(CONTAINER_TYPE)),
        text(trigger.text(CONTAINER)),
        text(trigger.text(FUNCTION_NAME)),
        number(trigger.u32(PRIORITY)),
        number(trigger.u32(OPTIONS)),
        text(trigger.comment.clone()),
        logical(trigger.has(TRIGGERS_DISABLED)),
    ]
}

fn user_row(user: &Object) -> Vec<Value> {
    use dd::user::*;
    vec![
        Value::Text(user.name.clone()),
        logical(user.u16(ENABLE_INTERNET) == Some(1)),
        logical(user.u16(LOGINS_DISABLED) == Some(1)),
        text(user.comment.clone()),
        binary(user.slot(USER_DEFINED)),
        logical(!user.has(NO_OLD_PASSWORD)),
    ]
}

fn group_rows(database: &Database) -> Vec<Vec<Value>> {
    let dd = database.dictionary();
    let mut rows: Vec<Vec<Value>> = dd
        .all(Kind::Group)
        .iter()
        .map(|group| {
            vec![
                Value::Text(group.name.clone()),
                text(group.comment.clone()),
                binary(group.slot(dd::group::USER_DEFINED)),
            ]
        })
        .collect();
    for built_in in dd::BUILT_IN_GROUPS {
        rows.push(vec![
            Value::Text(built_in.to_owned()),
            Value::Null,
            Value::Null,
        ]);
    }
    rows
}

/// The membership rows: each user's member groups (decrypted with the
/// dictionary key), then the built-in `DB:Public` every user belongs to, as the
/// engine lists them. A user's real groups need the dictionary key; where the
/// connection's password does not recover it, only `DB:Public` is listed (see
/// `docs/KNOWN_ISSUES.md`).
fn member_rows(database: &Database) -> Vec<Vec<Value>> {
    let dd = database.dictionary();
    let users = dd.all(Kind::User);
    let groups_of: Vec<Vec<String>> = users
        .iter()
        .map(|user| database.user_groups(user).unwrap_or_default())
        .collect();
    let row = |user: &Object, group: &str| {
        vec![
            Value::Text(user.name.clone()),
            Value::Text(group.to_owned()),
        ]
    };
    let mut rows = Vec::new();
    // By group, the real ones then the built-in ones (`DB:Public` with everyone
    // in it), the members of each in the order of the users
    // (`fixtures/sql/ddbuiltin`).
    for group in dd.all(Kind::Group) {
        for (user, groups) in users.iter().zip(&groups_of) {
            if groups.contains(&group.name) {
                rows.push(row(user, &group.name));
            }
        }
    }
    for built_in in dd::BUILT_IN_GROUPS {
        for user in &users {
            if built_in == "DB:Public" || database.built_in_groups(user).contains(&built_in) {
                rows.push(row(user, built_in));
            }
        }
    }
    rows
}

/// The built-in system groups, the same on every connection (no dictionary):
/// each with whether it can alter permissions and whether its membership can
/// be changed, in the original engine's order.
fn system_group_rows() -> Vec<Vec<Value>> {
    // (name, can alter permissions, can modify membership)
    const GROUPS: [(&str, bool, bool); 6] = [
        ("DB:Admin", false, true),
        ("DB:Debug", false, true),
        ("DB:Backup", false, true),
        ("DB:Public", true, false),
        ("SERVER:Admin", false, true),
        ("SERVER:Monitor", false, true),
    ];
    GROUPS
        .iter()
        .map(|&(name, alter, modify)| {
            vec![
                Value::Text(name.to_owned()),
                logical(alter),
                logical(modify),
            ]
        })
        .collect()
}

/// The permission bits (`ADS_PERMISSION_*`) each `system.permissions` column
/// reports. `Inherit` is handled apart (it is not a granted right but a computed
/// flag on a user's rows).
pub(crate) mod right {
    pub const READ: u32 = 0x0001;
    pub const UPDATE: u32 = 0x0002;
    pub const EXECUTE: u32 = 0x0004;
    /// `ADS_PERMISSION_INHERIT`: not a `system.permissions` column, but a bit
    /// `AdsDDGetPermissions` returns in a user's mask.
    pub const INHERIT: u32 = 0x0008;
    pub const INSERT: u32 = 0x0010;
    pub const DELETE: u32 = 0x0020;
    pub const ACCESS: u32 = 0x0040;
    pub const CREATE: u32 = 0x0080;
    pub const ALTER: u32 = 0x0100;
    pub const DROP: u32 = 0x0200;
}

/// The kind of object a `system.permissions` row is about, which fixes its
/// `Object_Type` and which rights apply to it.
#[derive(Clone, Copy, PartialEq)]
enum Cat {
    Table,
    Column,
    View,
    Procedure,
    Function,
    Package,
    Link,
    Publication,
    Subscription,
    User,
    Group,
    Database,
    /// A pseudo-object: the right to create an object of a kind (`TABLE`, ...).
    Create,
}

impl Cat {
    /// The `ADS_DD_*_OBJECT` type number the row reports.
    fn object_type(self, create_type: i64) -> i64 {
        match self {
            Cat::Table => 1,
            Cat::Column => 4,
            Cat::View => 6,
            Cat::User => 8,
            Cat::Group => 9,
            Cat::Procedure => 10,
            Cat::Database => 11,
            Cat::Link => 12,
            Cat::Publication => 15,
            Cat::Subscription => 17,
            Cat::Function => 18,
            Cat::Package => 19,
            // A create row's type is the kind it creates.
            Cat::Create => create_type,
        }
    }

    /// The permission bit the `column`th `system.permissions` right column means
    /// for this kind of object, or `None` where that column does not apply (the
    /// dump leaves it `NULL`). `column` indexes [`RIGHT_COLUMNS`].
    fn bit(self, column: usize) -> Option<u32> {
        let (name, bit) = RIGHT_COLUMNS[column];
        let applies = match self {
            Cat::Table | Cat::View => {
                matches!(
                    name,
                    "Select" | "Update" | "Insert" | "Delete" | "Alter" | "Drop"
                )
            }
            Cat::Column => matches!(name, "Select" | "Update" | "Insert"),
            Cat::Procedure | Cat::Function | Cat::Package => {
                matches!(name, "Execute" | "Alter" | "Drop")
            }
            Cat::Link => matches!(name, "Access" | "Alter" | "Drop"),
            Cat::User | Cat::Group | Cat::Publication | Cat::Subscription => {
                matches!(name, "Alter" | "Drop")
            }
            Cat::Database => name == "Alter",
            Cat::Create => name == "Create",
        };
        applies.then_some(bit)
    }
}

/// The right columns of `system.permissions`, in order, with the permission bit
/// each reads. `Inherit` carries bit 0 and is filled in separately.
const RIGHT_COLUMNS: [(&str, u32); 10] = [
    ("Select", right::READ),
    ("Update", right::UPDATE),
    ("Insert", right::INSERT),
    ("Delete", right::DELETE),
    ("Execute", right::EXECUTE),
    ("Access", right::ACCESS),
    ("Inherit", 0),
    ("Create", right::CREATE),
    ("Alter", right::ALTER),
    ("Drop", right::DROP),
];

/// A grantee a `system.permissions` row is for: a user, a group, or one of the
/// built-in groups (which are not stored objects and cannot be granted rights,
/// but have an implicit, fixed permission policy).
struct Grantee {
    name: String,
    /// The grantee's object id, for finding its grants; `None` for a built-in
    /// group, which has none.
    id: Option<u32>,
    is_user: bool,
    /// For a user in `system.effectivepermissions`, the ids of the groups it
    /// belongs to, whose grants it inherits; empty otherwise.
    group_ids: Vec<u32>,
    /// Whether a user's rights are those it holds through others too: the
    /// groups' it is in and `DB:Public`'s (`system.effectivepermissions`, and
    /// `AdsDDGetPermissions` asked for the inherited ones).
    effective: bool,
    /// Whether a user whose rights are effective is a member of `DB:Admin`,
    /// which holds what that group holds (`fixtures/sql/ddpublicbad6`).
    admin: bool,
}

/// An object (or create pseudo-object) a `system.permissions` row is about.
struct Target {
    cat: Cat,
    name: String,
    /// The `Parent` column: a column's table name, else `NULL`.
    parent: Option<String>,
    /// The object's id, for finding grants on it; `None` for a built-in group
    /// object and a create pseudo-object.
    id: Option<u32>,
    /// A column's table's id and the column's position in field order, to read
    /// the table's per-column grants.
    column_of: Option<(u32, usize)>,
    /// The `Object_Type` a create row reports (the kind it creates).
    create_type: i64,
    /// Whether this object is a built-in group (which has the fixed policy).
    built_in_group: bool,
}

/// `system.permissions`: a row for every grantee (each user, then each group and
/// the built-in groups) and every object of the dictionary (its tables and their
/// columns, views, procedures, functions, packages, links, publications,
/// subscriptions, users, groups and the database, then the right to create each
/// kind of object), as the original
/// engine builds the grid. Each cell is a level — 0 none, 1 granted, 2 held
/// implicitly by a built-in administrative group. Grants are decrypted with the
/// dictionary key; without it (no administrator password on the connection) the
/// default matrix is built and grants read as 0 (see `docs/KNOWN_ISSUES.md`).
fn permission_rows(database: &Database) -> Vec<Vec<Value>> {
    permission_matrix(database, false)
}

/// `system.effectivepermissions`: the same grid as [`permission_rows`], except a
/// user also holds the rights granted to the groups it belongs to (the engine
/// rolls a user's group memberships into its own rights).
fn effective_permission_rows(database: &Database) -> Vec<Vec<Value>> {
    permission_matrix(database, true)
}

/// The shared builder for `system.permissions` and `system.effectivepermissions`:
/// `effective` adds each user's groups' grants to its own.
fn permission_matrix(database: &Database, effective: bool) -> Vec<Vec<Value>> {
    let dd = database.dictionary();
    let key = database.key();
    let grantees = permission_grantees(database, effective);
    let targets = permission_targets(dd);
    let mut rows = Vec::with_capacity(grantees.len() * targets.len());
    for grantee in &grantees {
        for target in &targets {
            let mut row = vec![
                Value::Text(target.name.clone()),
                Value::Integer(target.cat.object_type(target.create_type) as i128),
                text(target.parent.clone()),
                Value::Text(grantee.name.clone()),
            ];
            for column in 0..RIGHT_COLUMNS.len() {
                row.push(permission_cell(dd, key.as_deref(), grantee, target, column));
            }
            rows.push(row);
        }
    }
    rows
}

/// For each row [`permission_matrix`] builds, whether the original reads a
/// `DB:Public` rights slot that does not check to make it: the row of
/// `DB:Public` itself, and of a user in the effective permissions, for an
/// object (not a column of its own: the table's slot) whose slot is bad.
fn permission_bad_rows(database: &Database, effective: bool) -> Vec<bool> {
    let dd = database.dictionary();
    let key = database.key();
    let grantees = permission_grantees(database, effective);
    let targets = permission_targets(dd);
    let holder_bad: Vec<bool> = targets
        .iter()
        .map(|target| {
            let holder = target.column_of.map(|(table_id, _)| table_id).or(target.id);
            target.cat != Cat::Create
                && holder.and_then(|id| dd.object(id)).is_some_and(|object| {
                    dd.public_rights(object, key.as_deref()) == dd::PublicRights::Invalid
                })
        })
        .collect();
    let mut bad = Vec::with_capacity(grantees.len() * targets.len());
    for grantee in &grantees {
        let public = grantee.id.is_none() && grantee.name == "DB:Public";
        for (target, holder_bad) in targets.iter().zip(&holder_bad) {
            let reads_public = public || (grantee.effective && target.column_of.is_none());
            bad.push(reads_public && *holder_bad);
        }
    }
    bad
}

/// The grantees of `system.permissions`, in the engine's order: each user, then
/// each real group, then the built-in groups. With `effective`, each user carries
/// the ids of the groups it belongs to (whose grants it then inherits).
fn permission_grantees(database: &Database, effective: bool) -> Vec<Grantee> {
    let dd = database.dictionary();
    let mut grantees: Vec<Grantee> = dd
        .all(Kind::User)
        .iter()
        .map(|user| Grantee {
            name: user.name.clone(),
            id: Some(user.id),
            is_user: true,
            group_ids: if effective {
                member_group_ids(database, user)
            } else {
                Vec::new()
            },
            effective,
            admin: effective && database.built_in_groups(user).contains(&"DB:Admin"),
        })
        .collect();
    for group in dd.all(Kind::Group) {
        grantees.push(Grantee {
            name: group.name.clone(),
            id: Some(group.id),
            is_user: false,
            group_ids: Vec::new(),
            effective: false,
            admin: false,
        });
    }
    for built_in in dd::BUILT_IN_GROUPS {
        grantees.push(Grantee {
            name: built_in.to_owned(),
            id: None,
            is_user: false,
            group_ids: Vec::new(),
            effective: false,
            admin: false,
        });
    }
    grantees
}

/// The object ids of the groups `user` belongs to, for the effective-permission
/// roll-up; empty when the membership cannot be read (no key).
fn member_group_ids(database: &Database, user: &Object) -> Vec<u32> {
    let dd = database.dictionary();
    database
        .user_groups(user)
        .unwrap_or_default()
        .iter()
        .filter_map(|name| dd.named(Kind::Group, name).map(|group| group.id))
        .collect()
}

/// Whether a group the user `user` is in holds rights on the table `table_id`
/// as a whole, with none of its columns apart: such a user is held to no column
/// rights of its own on the table, as the original engine does it
/// (`fixtures/sql/ddcolrights`).
pub(crate) fn group_holds_table_rights(database: &Database, user: &str, table_id: u32) -> bool {
    let dd = database.dictionary();
    let Some(key) = database.key() else {
        return false;
    };
    let Some(user) = dd.named(Kind::User, user) else {
        return false;
    };
    member_group_ids(database, user).into_iter().any(|group| {
        find_permission(dd, group, table_id).is_some_and(|permission| {
            permission.slot(dd::permission::COLUMN_RIGHTS).is_none()
                && rights_slot(dd.scheme(), &key, permission, dd::permission::OBJECT_RIGHTS)
                    .is_some_and(|mask| mask != 0)
        })
    })
}

/// Whether the user `user` or a group it is in holds rights on columns of the
/// table `table_id` apart from the table as a whole: only such a user is held
/// to column rights.
pub(crate) fn holds_column_rights(database: &Database, user: &str, table_id: u32) -> bool {
    let dd = database.dictionary();
    let Some(user) = dd.named(Kind::User, user) else {
        return false;
    };
    std::iter::once(user.id)
        .chain(member_group_ids(database, user))
        .any(|id| {
            find_permission(dd, id, table_id)
                .is_some_and(|p| p.slot(dd::permission::COLUMN_RIGHTS).is_some())
        })
}

/// The permission mask `AdsDDGetPermissions` returns for `grantee` on the object
/// (`object_type`, `object_name`, `parent_name` for a column): the bits it holds
/// (granted or implicit), plus `ADS_PERMISSION_INHERIT` for a user that holds it
/// (see [`user_inherits`]). `inherited` adds the grants of a user's groups, if it
/// inherits. Errors as the
/// engine does: an unknown grantee or object is [`Error::InvalidObjectName`], an
/// object type with no permissions [`Error::InvalidObjectType`].
pub(crate) fn permissions_mask(
    database: &Database,
    grantee: &str,
    object_type: u16,
    object_name: &str,
    parent_name: &str,
    inherited: bool,
) -> Result<u32, crate::Error> {
    let dd = database.dictionary();
    let (mut mask, grantee, target) = rights_at_level(
        database,
        grantee,
        object_type,
        object_name,
        parent_name,
        inherited,
        1,
    )?;
    // A user holds the inherit bit unless revoked (and never on a built-in group).
    if user_inherits(dd, database.key().as_deref(), &grantee, &target) {
        mask |= right::INHERIT;
    }
    Ok(mask)
}

/// The rights `user` holds on the object with the grant option, through its
/// groups too: all it may grant to others and take from them
/// (`fixtures/sql/ddgrantopt`).
pub(crate) fn grantable_mask(
    database: &Database,
    user: &str,
    object_type: u16,
    object_name: &str,
    parent_name: &str,
) -> Result<u32, crate::Error> {
    let (mask, ..) = rights_at_level(
        database,
        user,
        object_type,
        object_name,
        parent_name,
        true,
        2,
    )?;
    Ok(mask)
}

/// The bits `grantee` holds on the object at `level` (1 held, 2 with the grant
/// option), with who it is and what the object is.
fn rights_at_level(
    database: &Database,
    grantee: &str,
    object_type: u16,
    object_name: &str,
    parent_name: &str,
    inherited: bool,
    level: i128,
) -> Result<(u32, Grantee, Target), crate::Error> {
    let dd = database.dictionary();
    let grantee =
        resolve_grantee(database, grantee, inherited).ok_or(crate::Error::InvalidObjectName)?;
    let target = resolve_target(dd, object_type, object_name, parent_name)?;
    let key = database.key();
    // The original reads the object's `DB:Public` slot for whoever it counts
    // for and refuses one that does not check.
    let reads_public = (grantee.id.is_none() && grantee.name == "DB:Public")
        || (grantee.effective && target.column_of.is_none());
    if level == 1
        && reads_public
        && let Some(holder) = target.column_of.map(|(table_id, _)| table_id).or(target.id)
        && let Some(object) = dd.object(holder)
        && dd.public_rights(object, key.as_deref()) == dd::PublicRights::Invalid
    {
        return Err(crate::ace_error::sql(
            5127,
            "Inconsistency detected while reading stored property.",
        ));
    }
    let mut mask = 0u32;
    for column in 0..RIGHT_COLUMNS.len() {
        if let Some(bit) = target.cat.bit(column)
            && permission_level(dd, key.as_deref(), &grantee, &target, bit) >= level
        {
            mask |= bit;
        }
    }
    Ok((mask, grantee, target))
}

/// Whether the user `user` holds the right to create objects of the
/// `ADS_DD_*_OBJECT` type `create_type`, its own or through its groups.
pub(crate) fn holds_create_right(database: &Database, user: &str, create_type: i64) -> bool {
    create_right_level(database, user, create_type) >= 1
}

/// Whether `user` holds the right to create objects of `create_type` with the
/// grant option, so may give it to others and take it from them.
pub(crate) fn grants_create_right(database: &Database, user: &str, create_type: i64) -> bool {
    create_right_level(database, user, create_type) >= 2
}

fn create_right_level(database: &Database, user: &str, create_type: i64) -> i128 {
    let Some(grantee) = resolve_grantee(database, user, true) else {
        return 0;
    };
    let target = Target {
        cat: Cat::Create,
        name: String::new(),
        parent: None,
        id: None,
        column_of: None,
        create_type,
        built_in_group: false,
    };
    permission_level(
        database.dictionary(),
        database.key().as_deref(),
        &grantee,
        &target,
        0,
    )
}

/// The text of a group's rights properties (`ADS_DD_TABLES_RIGHTS` and the
/// others after it): `name:rights;` for each object of `kinds` (with the
/// `ADS_DD_*_OBJECT` type of each) the group has a Permission object for, the
/// rights as letters (`fixtures/errors/param_n`). `None` when the group has
/// rights but the key to read them cannot be recovered.
pub(crate) fn group_rights_text(
    database: &Database,
    group: &str,
    kinds: &[(Kind, u16)],
) -> Option<String> {
    let dd = database.dictionary();
    let Some(group) = dd.named(Kind::Group, group) else {
        return Some(String::new());
    };
    let mut text = String::new();
    for &(kind, object_type) in kinds {
        for object in dd.all(kind) {
            if find_permission(dd, group.id, object.id).is_none() {
                continue;
            }
            database.key()?;
            let mask =
                permissions_mask(database, &group.name, object_type, &object.name, "", false)
                    .ok()?;
            text.push_str(&format!("{}:{};", object.name, rights_letters(mask)));
        }
    }
    Some(text)
}

/// A rights mask as the letters of a group's rights properties: read, write
/// when update, insert and delete are all held (else `U`, `V` for insert and
/// `D`), execute. Alter and drop have none.
fn rights_letters(mask: u32) -> String {
    let mut letters = String::new();
    if mask & right::READ != 0 {
        letters.push('R');
    }
    let write = right::UPDATE | right::INSERT | right::DELETE;
    if mask & write == write {
        letters.push('W');
    } else {
        for (bit, letter) in [
            (right::UPDATE, 'U'),
            (right::INSERT, 'V'),
            (right::DELETE, 'D'),
        ] {
            if mask & bit != 0 {
                letters.push(letter);
            }
        }
    }
    if mask & right::EXECUTE != 0 {
        letters.push('X');
    }
    letters
}

/// The rights the user `user` effectively holds on the table `name`, for
/// access enforcement (probed, `tools/ace/dd/rights.c`): the table's own grants
/// OR'd with every one of its columns' — a grant on any column lets the user
/// reach the table at that right (a column `UPDATE` grant allows updating the
/// table, which has no per-column write enforcement). Rights roll up through the
/// user's groups and the built-in groups. The administrator and connections that
/// do not verify access rights do not call this (see [`Database::access_mask`]).
pub(crate) fn table_access_mask(database: &Database, user: &str, name: &str) -> u32 {
    let mut mask = permissions_mask(database, user, 1, name, "", true).unwrap_or(0);
    let dd = database.dictionary();
    if let Some(table) = dd.named(Kind::Table, name) {
        for field in dd.children(table.id, Kind::Field) {
            mask |= permissions_mask(database, user, 4, &field.name, name, true).unwrap_or(0);
        }
    }
    mask
}

/// Resolves a grantee name to a user, a real group or a built-in group, whatever
/// its case; `None` if there is no such grantee. With `inherited`, a user carries
/// its member-group ids (so its groups' grants count).
fn resolve_grantee(database: &Database, name: &str, inherited: bool) -> Option<Grantee> {
    let dd = database.dictionary();
    if let Some(user) = dd.named(Kind::User, name) {
        let group_ids = if inherited {
            member_group_ids(database, user)
        } else {
            Vec::new()
        };
        return Some(Grantee {
            name: user.name.clone(),
            id: Some(user.id),
            is_user: true,
            group_ids,
            effective: inherited,
            admin: inherited && database.built_in_groups(user).contains(&"DB:Admin"),
        });
    }
    if let Some(group) = dd.named(Kind::Group, name) {
        return Some(Grantee {
            name: group.name.clone(),
            id: Some(group.id),
            is_user: false,
            group_ids: Vec::new(),
            effective: false,
            admin: false,
        });
    }
    let built_in = dd::built_in_group(name)?;
    Some(Grantee {
        name: built_in.to_owned(),
        id: None,
        is_user: false,
        group_ids: Vec::new(),
        effective: false,
        admin: false,
    })
}

/// Resolves an object by its type and name (and a column's parent table) to a
/// [`Target`]. [`Error::InvalidObjectType`] for a type that has no permissions,
/// [`Error::InvalidObjectName`] for a type that does but no such object.
fn resolve_target(
    dd: &dd::Dictionary,
    object_type: u16,
    object_name: &str,
    parent_name: &str,
) -> Result<Target, crate::Error> {
    let found = || crate::Error::InvalidObjectName;
    let simple = |cat: Cat, kind: Kind| {
        dd.named_in_database(kind, object_name).map(|o| Target {
            cat,
            name: o.name.clone(),
            parent: None,
            id: Some(o.id),
            column_of: None,
            create_type: 0,
            built_in_group: false,
        })
    };
    match object_type {
        // Any kind, found by name (the original keeps names unique across them),
        // a built-in group's included; not a column nor the database.
        0 => [1, 6, 10, 18, 19, 12, 15, 17, 8, 9]
            .into_iter()
            .find_map(|object_type| resolve_target(dd, object_type, object_name, "").ok())
            .ok_or_else(found),
        1 => simple(Cat::Table, Kind::Table).ok_or_else(found),
        4 => {
            let table = dd.named(Kind::Table, parent_name).ok_or_else(found)?;
            let (position, field) = dd
                .children(table.id, Kind::Field)
                .into_iter()
                .enumerate()
                .find(|(_, f)| f.name.eq_ignore_ascii_case(object_name))
                .ok_or_else(found)?;
            Ok(Target {
                cat: Cat::Column,
                name: field.name.clone(),
                parent: Some(table.name.clone()),
                id: Some(field.id),
                column_of: Some((table.id, position)),
                create_type: 0,
                built_in_group: false,
            })
        }
        6 => simple(Cat::View, Kind::View).ok_or_else(found),
        10 => simple(Cat::Procedure, Kind::Procedure).ok_or_else(found),
        18 => simple(Cat::Function, Kind::Function).ok_or_else(found),
        19 => simple(Cat::Package, Kind::Package).ok_or_else(found),
        12 => simple(Cat::Link, Kind::Link).ok_or_else(found),
        15 => simple(Cat::Publication, Kind::Publication).ok_or_else(found),
        17 => simple(Cat::Subscription, Kind::Subscription).ok_or_else(found),
        8 => simple(Cat::User, Kind::User).ok_or_else(found),
        9 => simple(Cat::Group, Kind::Group)
            .or_else(|| {
                let built_in = dd::built_in_group(object_name)?;
                Some(Target {
                    cat: Cat::Group,
                    name: built_in.to_owned(),
                    parent: None,
                    id: None,
                    column_of: None,
                    create_type: 0,
                    built_in_group: true,
                })
            })
            .ok_or_else(found),
        11 => dd
            .database()
            .map(|db| Target {
                cat: Cat::Database,
                name: db.name.clone(),
                parent: None,
                id: Some(db.id),
                column_of: None,
                create_type: 0,
                built_in_group: false,
            })
            .ok_or_else(found),
        _ => Err(crate::Error::InvalidObjectType),
    }
}

/// The objects of `system.permissions`, in the engine's order: tables, then
/// their columns, views, procedures, functions, packages, links, publications,
/// subscriptions, users, groups (real then built-in), the database, then the
/// create pseudo-objects.
fn permission_targets(dd: &dd::Dictionary) -> Vec<Target> {
    let mut targets = Vec::new();
    let object = |cat: Cat, name: String, id: Option<u32>| Target {
        cat,
        name,
        parent: None,
        id,
        column_of: None,
        create_type: 0,
        built_in_group: false,
    };
    for table in dd.all(Kind::Table) {
        targets.push(object(Cat::Table, table.name.clone(), Some(table.id)));
    }
    for table in dd.all(Kind::Table) {
        for (position, field) in dd.children(table.id, Kind::Field).iter().enumerate() {
            targets.push(Target {
                cat: Cat::Column,
                name: field.name.clone(),
                parent: Some(table.name.clone()),
                id: Some(field.id),
                column_of: Some((table.id, position)),
                create_type: 0,
                built_in_group: false,
            });
        }
    }
    // Procedures and functions inside a package are not their own permission
    // objects, only the package is; the database's own are those directly under
    // it (the database object is every top-level object's parent).
    let database_id = dd.database().map(|db| db.id);
    for (kind, cat) in [
        (Kind::View, Cat::View),
        (Kind::Procedure, Cat::Procedure),
        (Kind::Function, Cat::Function),
        (Kind::Package, Cat::Package),
        (Kind::Link, Cat::Link),
        (Kind::Publication, Cat::Publication),
        (Kind::Subscription, Cat::Subscription),
        (Kind::User, Cat::User),
        (Kind::Group, Cat::Group),
    ] {
        let nested = matches!(cat, Cat::Procedure | Cat::Function);
        for object_of in dd.all(kind) {
            if nested && Some(object_of.parent) != database_id {
                continue;
            }
            targets.push(object(cat, object_of.name.clone(), Some(object_of.id)));
        }
    }
    for built_in in dd::BUILT_IN_GROUPS {
        targets.push(Target {
            built_in_group: true,
            ..object(Cat::Group, built_in.to_owned(), None)
        });
    }
    if let Some(db) = dd.database() {
        targets.push(object(Cat::Database, "Database".to_owned(), Some(db.id)));
    }
    // The right to create each kind of object, in the engine's fixed order.
    const CREATES: [(&str, i64); 10] = [
        ("TABLE", 1),
        ("VIEW", 6),
        ("USER", 8),
        ("USER GROUP", 9),
        ("PROCEDURE", 10),
        ("FUNCTION", 18),
        ("PACKAGE", 19),
        ("LINK", 12),
        ("PUBLICATION", 15),
        ("SUBSCRIPTION", 17),
    ];
    for (name, create_type) in CREATES {
        targets.push(Target {
            create_type,
            ..object(Cat::Create, name.to_owned(), None)
        });
    }
    targets
}

/// One cell of `system.permissions`: the level of the `column`th right for
/// `grantee` on `target`, or `NULL` where the right does not apply.
fn permission_cell(
    dd: &dd::Dictionary,
    key: Option<&[u8]>,
    grantee: &Grantee,
    target: &Target,
    column: usize,
) -> Value {
    if RIGHT_COLUMNS[column].0 == "Inherit" {
        return match permission_inherit(dd, key, grantee, target) {
            Some(level) => Value::Integer(level),
            None => Value::Null,
        };
    }
    let Some(bit) = target.cat.bit(column) else {
        return Value::Null;
    };
    Value::Integer(permission_level(dd, key, grantee, target, bit))
}

/// The `Inherit` flag for a user's row (see [`user_inherits`]); `None` on a
/// group's or a column's row, where the dump leaves it `NULL`.
fn permission_inherit(
    dd: &dd::Dictionary,
    key: Option<&[u8]>,
    grantee: &Grantee,
    target: &Target,
) -> Option<i128> {
    if !grantee.is_user || target.cat == Cat::Column {
        return None;
    }
    // A member of `DB:Admin` shows the group's level (`fixtures/sql/ddpublicbad6`).
    if grantee.admin && !target.built_in_group {
        return Some(2);
    }
    Some(user_inherits(dd, key, grantee, target).into())
}

/// Whether the user `grantee` inherits its groups' rights on `target`: the
/// `ADS_PERMISSION_INHERIT` bit of its own Permission object for the target (for
/// a column, its table's), which a new user permission starts with and a revoke
/// can clear (probed, `tools/ace/dd/pubperm.c`); with no Permission object, or
/// none readable, it does. For a create row, the inherit bit of the user's byte
/// for the kind (probed, `tools/ace/dd/grants.c`). Never a group, nor on a
/// built-in group object.
fn user_inherits(
    dd: &dd::Dictionary,
    key: Option<&[u8]>,
    grantee: &Grantee,
    target: &Target,
) -> bool {
    if !grantee.is_user || target.built_in_group {
        return false;
    }
    if target.cat == Cat::Create {
        return create_byte(dd, key, grantee.id, target.create_type)
            .is_none_or(|byte| byte & dd::permission::CREATE_INHERIT != 0);
    }
    let holder = target.column_of.map(|(table_id, _)| table_id).or(target.id);
    let (Some(user_id), Some(holder), Some(key)) = (grantee.id, holder, key) else {
        return true;
    };
    find_permission(dd, user_id, holder)
        .and_then(|permission| object_mask(dd.scheme(), key, permission))
        .is_none_or(|mask| mask & right::INHERIT != 0)
}

/// The level of one right (`bit`) for `grantee` on `target`: 2 where it is
/// granted with the grant option or a built-in administrative group holds it
/// implicitly, 1 where it is granted, else 0.
fn permission_level(
    dd: &dd::Dictionary,
    key: Option<&[u8]>,
    grantee: &Grantee,
    target: &Target,
    bit: u32,
) -> i128 {
    // A built-in group's rights are a fixed policy, not stored grants.
    let Some(grantee_id) = grantee.id else {
        if grantee.name == "DB:Public" {
            return public_level(dd, key, target, bit);
        }
        return built_in_level(&grantee.name, target, bit);
    };
    // A real grantee's right is its own or, for a user in
    // `system.effectivepermissions` that inherits on this object, any group's
    // it belongs to — whichever is highest.
    let groups: &[u32] = if user_inherits(dd, key, grantee, target) {
        &grantee.group_ids
    } else {
        &[]
    };
    // What `DB:Public` is granted on the object itself reaches every user
    // whether it inherits or not, but not the columns (`fixtures/dictionary/public`).
    let public = if grantee.effective && target.column_of.is_none() {
        public_level(dd, key, target, bit)
    } else {
        0
    };
    std::iter::once(grantee_id)
        .chain(groups.iter().copied())
        .map(|id| match (target.cat, target.column_of) {
            (Cat::Create, _) => create_level(dd, key, id, target.create_type),
            (_, Some((table_id, position))) => column_level(dd, key, id, table_id, position, bit),
            _ => target
                .id
                .map_or(0, |object_id| object_level(dd, key, id, object_id, bit)),
        })
        .max()
        .unwrap_or(0)
        .max(public)
        .max(if grantee.admin {
            built_in_level("DB:Admin", target, bit)
        } else {
            0
        })
}

/// The level of `bit` for the built-in `DB:Public` on `target`: 1 where the
/// object's own slot grants it (a column reads its table's), never with the
/// grant option; a slot that does not check grants nothing here.
fn public_level(dd: &dd::Dictionary, key: Option<&[u8]>, target: &Target, bit: u32) -> i128 {
    if target.cat == Cat::Create {
        return 0;
    }
    let holder = target.column_of.map(|(table_id, _)| table_id).or(target.id);
    let granted = holder
        .and_then(|id| dd.object(id))
        .map(|object| dd.public_rights(object, key));
    i128::from(matches!(granted, Some(dd::PublicRights::Rights(rights)) if rights & bit != 0))
}

/// The byte of the grantee `grantee_id`'s create rights for the kind
/// `create_type`, deciphered: `None` where it has none stored (or no key).
fn create_byte(
    dd: &dd::Dictionary,
    key: Option<&[u8]>,
    grantee_id: Option<u32>,
    create_type: i64,
) -> Option<u8> {
    use dd::permission::{CREATE_RIGHTS, create_index};
    let (key, object) = key.zip(grantee_id.and_then(|id| dd.object(id)))?;
    let index = create_index(u16::try_from(create_type).ok()?)?;
    let mut data = object.slot(CREATE_RIGHTS)?.to_vec();
    let item = tflip_storage::crypto::permission_item(object.id, CREATE_RIGHTS as u32);
    tflip_storage::crypto::crypt(dd.scheme(), key, item, &mut data);
    tflip_storage::crypto::permission_column_rights(&data)?
        .get(index)
        .copied()
}

/// The level of the right to create the kind `create_type` for `grantee_id`: 2
/// with the grant option, 1 without.
fn create_level(
    dd: &dd::Dictionary,
    key: Option<&[u8]>,
    grantee_id: u32,
    create_type: i64,
) -> i128 {
    use dd::permission::{CREATE, CREATE_WITH_GRANT};
    match create_byte(dd, key, Some(grantee_id), create_type) {
        Some(byte) if byte & CREATE_WITH_GRANT != 0 => 2,
        Some(byte) if byte & CREATE != 0 => 1,
        _ => 0,
    }
}

/// The fixed rights a built-in administrative group holds on `target`: `DB:Admin`
/// holds everything (level 2) but may only alter the other built-in groups (not
/// `DB:Public`); each built-in group may alter itself (not `DB:Public`); and
/// `DB:Debug` may alter procedures, functions and packages. Everything else is 0.
fn built_in_level(grantee: &str, target: &Target, bit: u32) -> i128 {
    if grantee == "DB:Admin" {
        if target.cat == Cat::Group && target.built_in_group {
            return i128::from(bit == right::ALTER && target.name != "DB:Public");
        }
        return 2;
    }
    let self_alter = target.cat == Cat::Group
        && target.built_in_group
        && target.name == grantee
        && grantee != "DB:Public"
        && bit == right::ALTER;
    let debug_alter = grantee == "DB:Debug"
        && matches!(target.cat, Cat::Procedure | Cat::Function | Cat::Package)
        && bit == right::ALTER;
    i128::from(self_alter || debug_alter)
}

/// The level of `bit` for `grantee_id` on the object `object_id`: 2 where its
/// Permission object holds it with the grant option (slot 3), 1 where it holds
/// it (slot 1), else 0.
fn object_level(
    dd: &dd::Dictionary,
    key: Option<&[u8]>,
    grantee_id: u32,
    object_id: u32,
    bit: u32,
) -> i128 {
    let Some((key, permission)) = key.zip(find_permission(dd, grantee_id, object_id)) else {
        return 0;
    };
    let held = |slot: usize| {
        rights_slot(dd.scheme(), key, permission, slot).is_some_and(|mask| mask & bit != 0)
    };
    if held(dd::permission::WITH_GRANT_RIGHTS) {
        2
    } else {
        held(dd::permission::OBJECT_RIGHTS).into()
    }
}

/// The level of `bit` for `grantee_id` on the `position`th column of `table_id`:
/// from the column's own byte where the table's Permission object keeps
/// per-column rights (slots 2 and 4), else from the table's rights reflected to
/// its columns (slots 1 and 3).
fn column_level(
    dd: &dd::Dictionary,
    key: Option<&[u8]>,
    grantee_id: u32,
    table_id: u32,
    position: usize,
    bit: u32,
) -> i128 {
    use dd::permission::{
        COLUMN_RIGHTS, OBJECT_RIGHTS, WITH_GRANT_COLUMN_RIGHTS, WITH_GRANT_RIGHTS,
    };
    let Some((key, permission)) = key.zip(find_permission(dd, grantee_id, table_id)) else {
        return 0;
    };
    let held = |columns: usize, rights: usize| match permission.slot(columns) {
        Some(blob) => {
            let mut data = blob.to_vec();
            let item = permission_item(permission, columns as u32);
            tflip_storage::crypto::crypt(dd.scheme(), key, item, &mut data);
            tflip_storage::crypto::permission_column_rights(&data)
                .and_then(|bytes| bytes.get(position).copied())
                .is_some_and(|byte| dd::permission::column_rights(byte) & bit != 0)
        }
        None => {
            rights_slot(dd.scheme(), key, permission, rights).is_some_and(|mask| mask & bit != 0)
        }
    };
    if held(WITH_GRANT_COLUMN_RIGHTS, WITH_GRANT_RIGHTS) {
        2
    } else {
        held(COLUMN_RIGHTS, OBJECT_RIGHTS).into()
    }
}

/// The decrypted rights mask in a Permission object's `slot` (1 or 3).
fn rights_slot(scheme: Scheme, key: &[u8], permission: &Object, slot: usize) -> Option<u32> {
    let blob = permission.slot(slot)?;
    let mut data = blob.to_vec();
    tflip_storage::crypto::crypt(
        scheme,
        key,
        permission_item(permission, slot as u32),
        &mut data,
    );
    tflip_storage::crypto::permission_rights(&data)
}

/// The decrypted object-rights mask of a Permission object (its slot 1).
fn object_mask(scheme: Scheme, key: &[u8], permission: &Object) -> Option<u32> {
    let blob = permission.slot(dd::permission::OBJECT_RIGHTS)?;
    let mut data = blob.to_vec();
    tflip_storage::crypto::crypt(scheme, key, permission_item(permission, 1), &mut data);
    tflip_storage::crypto::permission_rights(&data)
}

/// The cipher item a Permission object's `slot` is enciphered under.
fn permission_item(permission: &Object, slot: u32) -> u32 {
    tflip_storage::crypto::permission_item(permission.id, slot)
}

/// The Permission object granting `grantee_id` rights on `target_id`, if any.
fn find_permission(dd: &dd::Dictionary, grantee_id: u32, target_id: u32) -> Option<&Object> {
    dd.all(Kind::Permission)
        .into_iter()
        .find(|p| p.parent == grantee_id && p.info1 == Some(target_id as i32))
}

fn relation_row(database: &Database, rule: &Object) -> Vec<Value> {
    use dd::relation::*;
    let name = |slot| database.name_of(rule.u32(slot));
    vec![
        Value::Text(rule.name.clone()),
        text(name(PRIMARY_TABLE)),
        text(name(PRIMARY_INDEX)),
        text(name(FOREIGN_TABLE)),
        text(name(FOREIGN_INDEX)),
        number(rule.u8(UPDATE_RULE)),
        number(rule.u8(DELETE_RULE)),
        text(rule.text(NO_PKEY_ERROR)),
        text(rule.text(CASCADE_ERROR)),
    ]
}

fn link_row(database: &Database, link: &Object) -> Vec<Value> {
    use dd::link::*;
    // A user who may use a link is told its name alone.
    if !database.sees_everything() {
        return vec![
            Value::Text(link.name.clone()),
            Value::Null,
            Value::Null,
            Value::Null,
        ];
    }
    vec![
        Value::Text(link.name.clone()),
        text(link.text(RELATIVE_PATH)),
        number(link.info1.map(|o| o as i64)),
        text(link.text(USER_NAME)),
    ]
}

fn publication_row(publication: &Object) -> Vec<Value> {
    use dd::publication::*;
    vec![
        Value::Text(publication.name.clone()),
        text(publication.comment.clone()),
        number(publication.u32(OPTIONS)),
        binary(publication.slot(USER_DEFINED)),
    ]
}

/// The articles of each publication in turn, named by the table they publish
/// (`fixtures/sql/ddrepl`).
fn article_rows(database: &Database) -> Vec<Vec<Value>> {
    use dd::article::*;
    let dd = database.dictionary();
    let mut rows = Vec::new();
    for publication in dd.all(Kind::Publication) {
        for article in dd.children(publication.id, Kind::Article) {
            let table = article.info1.and_then(|id| dd.object(id as u32));
            // The include or exclude list, by the kind flag in slot 4.
            let list = |want: u16| {
                (article.u16(LIST_KIND) == Some(want))
                    .then(|| article.text(COLUMN_LIST))
                    .flatten()
            };
            rows.push(vec![
                Value::Text(table.map(|t| t.name.clone()).unwrap_or_default()),
                Value::Text(publication.name.clone()),
                text(article.text(ID_COLUMNS)),
                text(article.text(FILTER)),
                text(list(LIST_INCLUDE)),
                text(list(LIST_EXCLUDE)),
                logical(article.has(INSERT_MERGE)),
                logical(article.has(UPDATE_MERGE)),
            ]);
        }
    }
    rows
}

fn subscription_row(database: &Database, subscription: &Object) -> Vec<Value> {
    use dd::subscription::{comm, *};
    let dd = database.dictionary();
    let options = subscription.u32(OPTIONS).unwrap_or(0);
    let comm_bit = |bit: u32| logical(options & bit != 0);
    // The connection string is kept enciphered, as the property reads it.
    let connection = database
        .property(
            &crate::dictionary::ObjectRef::Subscription(subscription.name.clone()),
            1716,
        )
        .ok()
        .and_then(|p| match p {
            crate::dictionary::Property::Text(text) => Some(text),
            _ => None,
        });
    vec![
        Value::Text(subscription.name.clone()),
        Value::Text(
            subscription
                .info1
                .and_then(|id| dd.object(id as u32))
                .map(|p| p.name.clone())
                .unwrap_or_default(),
        ),
        text(subscription.text(TARGET)),
        text(subscription.text(USER_NAME)),
        text(subscription.text(QUEUE_NAME)),
        logical(subscription.u16(FORWARD) == Some(1)),
        logical(subscription.u16(ENABLED) == Some(1)),
        text(subscription.comment.clone()),
        number(Some(options & !comm::MASK)),
        binary(subscription.slot(USER_DEFINED)),
        logical(subscription.u16(PAUSED) == Some(1)),
        comm_bit(comm::TCP_IP),
        comm_bit(comm::TCP_IP_V6),
        comm_bit(comm::UDP_IP),
        comm_bit(comm::IPX),
        comm_bit(comm::TLS),
        text(connection),
    ]
}

fn package_row(package: &Object) -> Vec<Value> {
    vec![
        Value::Text(package.name.clone()),
        number(package.info1.map(i64::from).or(Some(0))),
        text(package.comment.clone()),
        binary(package.slot(dd::field::USER_DEFINED)),
    ]
}

/// A user-defined property, as the bytes the dictionary keeps, or `NULL`.
fn binary(slot: Option<&[u8]>) -> Value {
    slot.map_or(Value::Null, |bytes| Value::Binary(bytes.to_vec()))
}

/// The `system.fts` row: the dictionary's full-text settings, each falling
/// back to the engine's default where the dictionary has not set it.
fn fts_row(database: Option<&Database>) -> Vec<Value> {
    use dd::database::*;
    let db = database.and_then(|d| d.dictionary().database());
    let setting = |slot, default: &str| {
        let value = db.and_then(|db| db.text(slot));
        Value::Text(value.unwrap_or_else(|| default.to_owned()))
    };
    let delimiters = db
        .and_then(|db| db.text(FTS_DELIMITERS))
        .unwrap_or_else(|| String::from_utf8_lossy(fts_default::DELIMITERS).into_owned());
    vec![
        Value::Text(delimiters),
        setting(FTS_NOISE, fts_default::NOISE),
        setting(FTS_DROP_CHARS, fts_default::DROP_CHARS),
        setting(FTS_CONDITIONAL_CHARS, fts_default::CONDITIONAL_CHARS),
    ]
}

/// The system tables, in the order `system.objects` lists them and
/// `sp_GetTables` returns them: each name, the table it details, and the
/// orders a client could open it by.
pub(crate) const SYSTEM_TABLE_CATALOG: &[(&str, &str, &str, &str)] = &[
    ("dictionary", "", "", ""),
    ("tables", "", "Name", "Name"),
    ("columns", "tables", "Parent,Field_Num", "Parent,Name"),
    ("users", "", "Name", "Name"),
    ("usergroups", "", "Name", "Name"),
    ("usergroupmembers", "", "User_Name", "Group_Name,User_Name"),
    ("indexfiles", "tables", "Name", "Parent,Name"),
    ("indexes", "tables", "Name", "Parent,Index_File_Name,Name"),
    ("permissions", "", "Object_Type,Name", "Grantee,Parent,Name"),
    ("relations", "", "Name", "Name"),
    ("views", "", "Name", "Name"),
    ("storedprocedures", "", "Name", "Name"),
    ("links", "", "Name", "Name"),
    ("triggers", "tables", "Name", "Trig_TableName,Name"),
    ("fts", "", "", ""),
    (
        "effectivepermissions",
        "",
        "Object_Type,Name",
        "Grantee,Parent,Name",
    ),
    ("publications", "", "Name", "Name"),
    (
        "publicationarticles",
        "publications",
        "Parent,Name",
        "Parent,Name",
    ),
    ("subscriptions", "", "Name", "Name"),
    ("functions", "packages", "Package,Name", "Package,Name"),
    ("packages", "", "Name", "Name"),
];

/// The rows of `system.systemprocedures`: the system procedures with their
/// parameters, comments and the permissions they need, which any connection
/// reads.
fn system_procedure_rows() -> Vec<Vec<Value>> {
    let memo = |text: Option<&str>| text.map_or(Value::Null, |t| Value::Text(t.to_owned()));
    crate::procedure::system_procedures()
        .iter()
        .map(|p| {
            vec![
                Value::Text(p.name.to_owned()),
                memo(p.input),
                memo(p.output),
                memo(p.comment),
                Value::Integer(i128::from(p.permissions)),
            ]
        })
        .collect()
}

/// The catalog `system.objects` lists: each system table, the table it
/// details, and the orders a client could open it by.
fn objects_rows() -> Vec<Vec<Value>> {
    const CATALOG: &[(&str, &str, &str, &str)] = SYSTEM_TABLE_CATALOG;
    let cell = |s: &str| {
        if s.is_empty() {
            Value::Null
        } else {
            Value::Text(s.to_owned())
        }
    };
    CATALOG
        .iter()
        .map(|&(name, parent, order, key)| {
            vec![
                Value::Text(name.to_owned()),
                cell(parent),
                cell(order),
                cell(key),
            ]
        })
        .collect()
}

/// A system table's columns, by the ACE field type each has.
fn columns_of(columns: &[(&str, u16, u16, u16)]) -> Vec<Column> {
    columns
        .iter()
        .map(|&(name, field_type, length, decimals)| Column {
            name: name.to_owned(),
            field_type: tflip_storage::field::FieldType::from_code(field_type),
            length,
            decimals,
        })
        .collect()
}

/// The columns of the system table `name`, if it is one (whatever its
/// case). Each column is `(name, ACE field type, length, decimals)`, as the
/// original engine's cursors report them.
fn schema(name: &str) -> Option<Vec<(&'static str, u16, u16, u16)>> {
    let columns: &[(&str, u16, u16, u16)] = match name.to_ascii_lowercase().as_str() {
        "dictionary" => &DICTIONARY,
        "objects" => &OBJECTS,
        "tables" => &TABLES,
        "columns" => &COLUMNS,
        "indexfiles" => &INDEXFILES,
        "indexes" => &INDEXES,
        "views" => &VIEWS,
        "storedprocedures" => &STOREDPROCEDURES,
        "functions" => &FUNCTIONS,
        "triggers" => &TRIGGERS,
        "users" => &USERS,
        "usergroups" => &USERGROUPS,
        "usergroupmembers" => &USERGROUPMEMBERS,
        "systemgroups" => &SYSTEMGROUPS,
        "systemprocedures" => &SYSTEMPROCEDURES,
        "permissions" => &PERMISSIONS,
        "effectivepermissions" => &PERMISSIONS,
        "relations" => &RELATIONS,
        "links" => &LINKS,
        "packages" => &PACKAGES,
        "publications" => &PUBLICATIONS,
        "publicationarticles" | "articles" => &PUBLICATIONARTICLES,
        "subscriptions" => &SUBSCRIPTIONS,
        "fts" => &FTS,
        _ => return None,
    };
    Some(columns.to_vec())
}

// The schemas, taken from the original engine's cursors (ACE field types:
// 1 Logical, 5 Memo, 6 Binary, 11 Integer, 12 ShortInt, 20 CICharacter,
// 28 NMemo).

const DICTIONARY: [(&str, u16, u16, u16); 28] = [
    ("Version_Major", 12, 2, 0),
    ("Version_Minor", 12, 2, 0),
    ("Default_Table_Path", 20, 260, 0),
    ("Temp_Table_Path", 20, 260, 0),
    ("Log_In_Required", 1, 1, 0),
    ("Verify_Access_Rights", 1, 1, 0),
    ("Encrypt_Table_Password", 20, 20, 0),
    ("Encrypt_New_Table", 1, 1, 0),
    ("Enable_Internet", 1, 1, 0),
    ("Internet_Security_Level", 12, 2, 0),
    ("Max_Failed_Attempts", 12, 2, 0),
    ("Logins_Disabled", 1, 1, 0),
    ("Logins_Disabled_Msg", 5, 9, 0),
    ("Comment", 5, 9, 0),
    ("User_Defined_Prop", 6, 9, 0),
    ("FTS_Delimiters", 28, 9, 0),
    ("FTS_Noise_Words", 28, 9, 0),
    ("FTS_Drop_Chars", 28, 9, 0),
    ("FTS_Conditional_Chars", 28, 9, 0),
    ("Dictionary_Encrypted", 1, 1, 0),
    ("Encrypt_Indexes", 1, 1, 0),
    ("Query_Log_Table", 20, 260, 0),
    ("Encrypt_Communication", 1, 1, 0),
    ("Disable_DLL_Caching", 1, 1, 0),
    ("Triggers_Disabled", 1, 1, 0),
    ("Data_Encryption_Type", 11, 4, 0),
    ("Query_Via_Root", 11, 4, 0),
    ("Enforce_Max_Failed_Logins", 1, 1, 0),
];

const OBJECTS: [(&str, u16, u16, u16); 4] = [
    ("System_Table", 20, 200, 0),
    ("Parent_Table", 20, 200, 0),
    ("Order_By", 20, 200, 0),
    ("Primary_Key", 20, 200, 0),
];

const TABLES: [(&str, u16, u16, u16); 18] = [
    ("Name", 20, 200, 0),
    ("Table_Relative_Path", 20, 260, 0),
    ("Table_Type", 12, 2, 0),
    ("Table_Auto_Create", 1, 1, 0),
    ("Table_Primary_Key", 20, 128, 0),
    ("Table_Default_Index", 20, 128, 0),
    ("Table_Encryption", 1, 1, 0),
    ("Table_Permission_Level", 12, 2, 0),
    ("Table_Memo_Block_Size", 12, 2, 0),
    ("Table_Validation_Expr", 28, 9, 0),
    ("Table_Validation_Msg", 5, 9, 0),
    ("Comment", 5, 9, 0),
    ("User_Defined_Prop", 6, 9, 0),
    ("Triggers_Disabled", 1, 1, 0),
    ("Table_Caching", 12, 2, 0),
    ("Table_Trans_Free", 1, 1, 0),
    ("Table_WEB_delta", 1, 1, 0),
    ("Table_Concurrency_Enabled", 1, 1, 0),
];

const COLUMNS: [(&str, u16, u16, u16); 14] = [
    ("Name", 20, 200, 0),
    ("Parent", 20, 200, 0),
    ("Field_Num", 11, 4, 0),
    ("Field_Type", 12, 2, 0),
    ("Field_Length", 11, 4, 0),
    ("Field_Decimal", 12, 2, 0),
    ("Field_Min_Value", 28, 9, 0),
    ("Field_Max_Value", 28, 9, 0),
    ("Field_Can_Be_Null", 1, 1, 0),
    ("Field_Default_Value", 28, 9, 0),
    ("Field_Validation_Msg", 5, 9, 0),
    ("Comment", 5, 9, 0),
    ("User_Defined_Prop", 6, 9, 0),
    ("Field_Options", 11, 4, 0),
];

const INDEXFILES: [(&str, u16, u16, u16); 4] = [
    ("Name", 20, 200, 0),
    ("Parent", 20, 200, 0),
    ("Index_File_Path", 20, 260, 0),
    ("Index_File_PageSize", 11, 4, 0),
];

const INDEXES: [(&str, u16, u16, u16); 14] = [
    ("Name", 20, 200, 0),
    ("Parent", 20, 200, 0),
    ("Index_File_Name", 20, 200, 0),
    ("Index_Expression", 20, 510, 0),
    ("Index_Condition", 20, 510, 0),
    ("Index_Options", 11, 4, 0),
    ("Index_Key_Length", 12, 2, 0),
    ("Index_FTS_Min_Length", 12, 2, 0),
    ("Index_FTS_Delimiters", 28, 9, 0),
    ("Index_FTS_Noise", 28, 9, 0),
    ("Index_FTS_Drop_Chars", 28, 9, 0),
    ("Index_FTS_Conditional_Chars", 28, 9, 0),
    ("Comment", 5, 9, 0),
    ("Index_Collation", 20, 40, 0),
];

const VIEWS: [(&str, u16, u16, u16); 5] = [
    ("Name", 20, 200, 0),
    ("View_Stmt_Len", 12, 2, 0),
    ("View_Stmt", 28, 9, 0),
    ("Comment", 5, 9, 0),
    ("Triggers_Disabled", 1, 1, 0),
];

const STOREDPROCEDURES: [(&str, u16, u16, u16); 8] = [
    ("Name", 20, 200, 0),
    ("Proc_Input", 5, 9, 0),
    ("Proc_Output", 5, 9, 0),
    ("Proc_DLL_Name", 20, 260, 0),
    ("Proc_DLL_Function_Name", 20, 260, 0),
    ("Comment", 5, 9, 0),
    ("Proc_Invoke_Option", 11, 4, 0),
    ("SQL_Script", 28, 9, 0),
];

const FUNCTIONS: [(&str, u16, u16, u16); 7] = [
    ("Name", 20, 200, 0),
    ("Package", 20, 200, 0),
    ("Return Type", 20, 20, 0),
    ("Input Parameters", 5, 9, 0),
    ("Implementation", 28, 9, 0),
    ("Comment", 5, 9, 0),
    ("User_Defined_Prop", 6, 9, 0),
];

const TRIGGERS: [(&str, u16, u16, u16); 11] = [
    ("Name", 20, 200, 0),
    ("Trig_TableName", 20, 200, 0),
    ("Trig_Event_Type", 11, 4, 0),
    ("Trig_Trigger_Type", 11, 4, 0),
    ("Trig_Container_Type", 11, 4, 0),
    ("Trig_Container", 28, 9, 0),
    ("Trig_Function_Name", 20, 260, 0),
    ("Trig_Priority", 11, 4, 0),
    ("Trig_Options", 11, 4, 0),
    ("Comment", 5, 9, 0),
    ("Triggers_Disabled", 1, 1, 0),
];

const USERS: [(&str, u16, u16, u16); 6] = [
    ("Name", 20, 50, 0),
    ("Enable_Internet", 1, 1, 0),
    ("Logins_Disabled", 1, 1, 0),
    ("Comment", 5, 9, 0),
    ("User_Defined_Prop", 6, 9, 0),
    ("Require_Old_Password", 1, 1, 0),
];

const USERGROUPS: [(&str, u16, u16, u16); 3] = [
    ("Name", 20, 100, 0),
    ("Comment", 5, 9, 0),
    ("User_Defined_Prop", 6, 9, 0),
];

const USERGROUPMEMBERS: [(&str, u16, u16, u16); 2] =
    [("User_Name", 20, 50, 0), ("Group_Name", 20, 100, 0)];

const SYSTEMGROUPS: [(&str, u16, u16, u16); 3] = [
    ("Name", 20, 100, 0),
    ("Can_Alter_Permissions", 1, 1, 0),
    ("Can_Modify_Membership", 1, 1, 0),
];

const SYSTEMPROCEDURES: [(&str, u16, u16, u16); 5] = [
    ("Name", 20, 200, 0),
    ("Proc_Input", 5, 9, 0),
    ("Proc_Output", 5, 9, 0),
    ("Comment", 5, 9, 0),
    ("Required_Permissions", 11, 4, 0),
];

const PERMISSIONS: [(&str, u16, u16, u16); 14] = [
    ("Name", 20, 200, 0),
    ("Object_Type", 11, 4, 0),
    ("Parent", 20, 200, 0),
    ("Grantee", 20, 100, 0),
    ("Select", 11, 4, 0),
    ("Update", 11, 4, 0),
    ("Insert", 11, 4, 0),
    ("Delete", 11, 4, 0),
    ("Execute", 11, 4, 0),
    ("Access", 11, 4, 0),
    ("Inherit", 11, 4, 0),
    ("Create", 11, 4, 0),
    ("Alter", 11, 4, 0),
    ("Drop", 11, 4, 0),
];

const RELATIONS: [(&str, u16, u16, u16); 9] = [
    ("Name", 20, 200, 0),
    ("RI_Primary_Table", 20, 200, 0),
    ("RI_Primary_Index", 20, 200, 0),
    ("RI_Foreign_Table", 20, 200, 0),
    ("RI_Foreign_Index", 20, 200, 0),
    ("RI_UpdateRule", 12, 2, 0),
    ("RI_DeleteRule", 12, 2, 0),
    ("RI_No_PKey_Error", 5, 9, 0),
    ("RI_Cascade_Error", 5, 9, 0),
];

const LINKS: [(&str, u16, u16, u16); 4] = [
    ("Name", 20, 200, 0),
    ("Link_Path", 20, 260, 0),
    ("Link_Options", 11, 4, 0),
    ("Link_UserName", 20, 200, 0),
];

const PACKAGES: [(&str, u16, u16, u16); 4] = [
    ("Name", 20, 200, 0),
    ("Options", 11, 4, 0),
    ("Comment", 5, 9, 0),
    ("User_Defined_Prop", 6, 9, 0),
];

const PUBLICATIONS: [(&str, u16, u16, u16); 4] = [
    ("Name", 20, 200, 0),
    ("Comment", 5, 9, 0),
    ("Options", 11, 4, 0),
    ("User_Defined_Prop", 6, 9, 0),
];

const PUBLICATIONARTICLES: [(&str, u16, u16, u16); 8] = [
    ("Name", 20, 200, 0),
    ("Parent", 20, 200, 0),
    ("Ident_Columns", 5, 9, 0),
    ("Filter", 28, 9, 0),
    ("Include_Columns", 5, 9, 0),
    ("Exclude_Columns", 5, 9, 0),
    ("INSERT_MERGE", 1, 1, 0),
    ("UPDATE_MERGE", 1, 1, 0),
];

const SUBSCRIPTIONS: [(&str, u16, u16, u16); 17] = [
    ("Name", 20, 200, 0),
    ("Publication", 20, 200, 0),
    ("Target", 20, 260, 0),
    ("Target_UserName", 20, 200, 0),
    ("Replication_Queue", 20, 260, 0),
    ("Forward", 1, 1, 0),
    ("Enabled", 1, 1, 0),
    ("Comment", 5, 9, 0),
    ("Options", 11, 4, 0),
    ("User_Defined_Prop", 6, 9, 0),
    ("Paused", 1, 1, 0),
    ("TCP/IP", 1, 1, 0),
    ("TCP/IP_V6", 1, 1, 0),
    ("UDP/IP", 1, 1, 0),
    ("IPX", 1, 1, 0),
    ("TLS", 1, 1, 0),
    ("Connection_String", 5, 9, 0),
];

const FTS: [(&str, u16, u16, u16); 4] = [
    ("FTS_Delimiters", 28, 9, 0),
    ("FTS_Noise_Words", 28, 9, 0),
    ("FTS_Drop_Chars", 28, 9, 0),
    ("FTS_Conditional_Chars", 28, 9, 0),
];

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;

    use serde_json::Value as Json;

    use super::*;

    /// A built value as a `<dict>.system.json` dump shows it: text with its
    /// trailing spaces trimmed, `None` for NULL.
    fn shown(value: &Value) -> Option<String> {
        match value {
            Value::Null => None,
            Value::Text(text) => Some(text.trim_end().to_owned()),
            Value::Logical(b) => Some(if *b { "T" } else { "F" }.to_owned()),
            Value::Integer(n) => Some(n.to_string()),
            Value::Binary(bytes) => Some(bytes.iter().map(|b| format!("{b:02x}")).collect()),
            other => panic!("unexpected value {other:?}"),
        }
    }

    /// The dump's value for a column: its trimmed text, or `None` for NULL.
    fn dumped(value: &Json) -> Option<String> {
        value["text"].as_str().map(|t| t.trim_end().to_owned())
    }

    /// Every system table tflip builds matches the dictionary's `system.*`
    /// dump, column for column, for every dictionary fixture. With the
    /// administrator password supplied below, the encrypted tables
    /// (`usergroupmembers`, `permissions`, `effectivepermissions`) are read and
    /// checked like the rest.
    #[test]
    fn system_tables_match_the_dumps() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt");
        for (folder, name) in [
            ("dictionary", "dict"),
            ("psm", "psm"),
            ("dictionary_props", "props"),
        ] {
            let dir = root.join(folder);
            let mut database = Database::open(&dir.join(format!("{name}.add"))).unwrap();
            // Every fixture's administrator password, which recovers the key
            // that decrypts memberships (see `tools/ace/fixtures/*.sql`).
            database.set_password(b"adssyspw");
            let json = std::fs::read_to_string(dir.join(format!("{name}.system.json"))).unwrap();
            let system: Json = serde_json::from_str(&json).unwrap();
            for (table, dump) in system.as_object().unwrap() {
                let table = table.strip_prefix("system.").unwrap();
                let built = build(Some(&database), table).unwrap().unwrap();
                let columns: Vec<&str> = built.columns.iter().map(|c| c.name.as_str()).collect();
                let want_columns: Vec<&str> = dump["fields"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|f| f["name"].as_str().unwrap())
                    .collect();
                assert_eq!(columns, want_columns, "{name}.{table} columns");
                let records = dump["records"].as_array().unwrap();
                assert_eq!(built.rows.len(), records.len(), "{name}.{table} row count");
                for (row, record) in built.rows.iter().zip(records) {
                    let values = record["values"].as_object().unwrap();
                    let dump: HashMap<&str, Option<String>> = values
                        .iter()
                        .map(|(k, v)| (k.as_str(), dumped(v)))
                        .collect();
                    for (column, value) in columns.iter().zip(row) {
                        assert_eq!(shown(value), dump[column], "{name}.{table}.{column}");
                    }
                }
            }
        }
    }

    /// A `SELECT` reads a `system.*` table into a static cursor, with
    /// `WHERE` and `ORDER BY` over its columns, through the connection.
    #[test]
    fn select_reads_a_system_table() {
        use crate::{connection, statement, table};
        let session = crate::Session::new();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt/dictionary_props");
        let cs = format!(
            "Data Source={};User ID=ADSSYS;Password=adssyspw;",
            dir.join("props.add").display()
        );
        let connection = connection::connect(&session, &cs).unwrap();
        let statement = statement::create(&session, connection).unwrap();
        let count = |sql: &str| {
            let cursor = statement::execute_direct(&session, statement, sql, true)
                .unwrap()
                .unwrap();
            let count = table::get(&session, cursor)
                .unwrap()
                .record_count()
                .unwrap();
            table::close(&session, cursor).unwrap();
            count
        };
        assert_eq!(count("SELECT * FROM system.tables"), 4);
        assert_eq!(count("SELECT * FROM SYSTEM.COLUMNS"), 15);
        assert_eq!(
            count("SELECT Name FROM system.tables WHERE Table_Caching > 0 ORDER BY Name"),
            2
        );
    }

    /// `AdsDDGetPermissions` masks match the original engine (probed against the
    /// dict and props fixtures with the administrator password "adssyspw").
    #[test]
    fn permission_masks_match_the_engine() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt");
        let open = |folder: &str, name: &str| {
            let mut db = Database::open(&root.join(folder).join(format!("{name}.add"))).unwrap();
            db.set_password(b"adssyspw");
            db
        };
        let mask = |db: &Database, g: &str, ot: u16, name: &str, parent: &str, inh: bool| {
            permissions_mask(db, g, ot, name, parent, inh)
        };

        let dict = open("dictionary", "dict");
        // A user carries the inherit bit; its group's grant appears only when
        // inherited is asked for.
        assert_eq!(
            mask(&dict, "alice", 1, "customers", "", false).unwrap(),
            0x08
        );
        assert_eq!(
            mask(&dict, "alice", 1, "customers", "", true).unwrap(),
            0x09
        );
        assert_eq!(
            mask(&dict, "staff", 1, "customers", "", false).unwrap(),
            0x01
        );
        // Columns carry the inherit bit too (unlike the system table), and reflect
        // the table's grant.
        assert_eq!(
            mask(&dict, "alice", 4, "name", "customers", false).unwrap(),
            0x08
        );
        assert_eq!(
            mask(&dict, "alice", 4, "name", "customers", true).unwrap(),
            0x09
        );
        assert_eq!(
            mask(&dict, "staff", 4, "name", "customers", false).unwrap(),
            0x01
        );
        // Built-in groups: their implicit policy, no inherit bit.
        assert_eq!(
            mask(&dict, "DB:Admin", 1, "customers", "", false).unwrap(),
            0x333
        );
        assert_eq!(
            mask(&dict, "DB:Public", 1, "customers", "", false).unwrap(),
            0x00
        );
        assert_eq!(
            mask(&dict, "DB:Admin", 9, "DB:Backup", "", false).unwrap(),
            0x100
        );
        assert_eq!(
            mask(&dict, "DB:Debug", 10, "add_order", "", false).unwrap(),
            0x100
        );
        // A user has no inherit bit on a built-in group object.
        assert_eq!(
            mask(&dict, "alice", 9, "DB:Admin", "", false).unwrap(),
            0x00
        );

        // Errors, as the engine returns them.
        assert!(matches!(
            mask(&dict, "nobody", 1, "customers", "", false),
            Err(crate::Error::InvalidObjectName)
        ));
        assert!(matches!(
            mask(&dict, "alice", 1, "no_such_table", "", false),
            Err(crate::Error::InvalidObjectName)
        ));
        assert!(matches!(
            mask(&dict, "alice", 99, "customers", "", false),
            Err(crate::Error::InvalidObjectType)
        ));

        let props = open("dictionary_props", "props");
        // A table grant (UPDATE) plus the user's inherit bit.
        assert_eq!(mask(&props, "bob", 1, "parent", "", false).unwrap(), 0x0a);
        // An explicit per-column grant (UPDATE on `label`) plus inherit.
        assert_eq!(
            mask(&props, "bob", 4, "label", "parent", false).unwrap(),
            0x0a
        );
        // A group's table grant reflected: READ | INSERT, no inherit.
        assert_eq!(
            mask(&props, "readers", 1, "parent", "", false).unwrap(),
            0x11
        );
    }

    /// `system.objects` and `system.fts` build without a dictionary; the
    /// others need one.
    #[test]
    fn system_tables_without_a_dictionary() {
        assert_eq!(build(None, "objects").unwrap().unwrap().rows.len(), 21);
        let fts = build(None, "fts").unwrap().unwrap();
        assert_eq!(fts.rows.len(), 1);
        assert!(matches!(&fts.rows[0][1], Value::Text(noise) if noise.contains("because")));
        assert!(build(None, "tables").is_err(), "needs a dictionary");
        assert!(
            build(None, "nosuch").unwrap().is_none(),
            "not a system table"
        );
    }
}
