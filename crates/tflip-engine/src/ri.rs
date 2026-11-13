//! Referential integrity: a dictionary's `Kind::Relation` rules, enforced on
//! the tables they bind, as the original engine enforces them (probed,
//! `tools/ace/dd/ri.c` → `fixtures/dictionary/ri.json`).
//!
//! A rule ties a child table's foreign-key index to a parent table's primary
//! index, with an update rule and a delete rule. On the child's own insert or
//! update, a non-NULL foreign key must have a matching parent key, else the
//! write is refused ([`Error::NoParentKey`]: 7076 through the table API, the
//! AQE error 7200 through SQL). On the parent's own update of the key or delete
//! of a row, each rule applies its action to the children that point at the old
//! key:
//!
//! - **RESTRICT** refuses while any child points at it ([`Error::RiRestrict`]).
//! - **CASCADE** deletes the children (on a delete) or re-points them at the
//!   new key (on a key update).
//! - **SET NULL** sets the children's foreign key to NULL.
//! - **SET DEFAULT** sets it to the child field's default, which must then
//!   itself be a valid parent key (so a default with no parent row fails).
//!
//! The actions are run as the dictionary owner (like a trigger body), so a
//! connected user's access rights do not refuse them, and inside the
//! connection's transaction so a failed cascade rolls back with the statement.
//! Enforcement is shared by the table API (`crate::trigger::write_record_api`
//! and `delete_record_api`) and SQL DML (`dml.rs`): the two differ only in
//! the error code, and that a RESTRICT refusal of a
//! table-API delete is swallowed (the original returns success and leaves the
//! row), where SQL surfaces it.

use tflip_sql::{SqlError, V};
use tflip_storage::dd::{Kind, relation};
use tflip_storage::field::Field;

use crate::connection;
use crate::handle::Handle;
use crate::seek::{SeekType, SeekValue};
use crate::setters::SetValue;
use crate::table::TableSettings;
use crate::{Error, Session, index, statement, table};

thread_local! {
    /// How many rules are applying their actions on this thread.
    static APPLYING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Whether a rule is applying its action on this thread: what it writes is
/// the rule's, not the statement's.
pub(crate) fn active() -> bool {
    APPLYING.with(|depth| depth.get() > 0)
}

/// Marks the thread as applying a rule while it lives.
struct Applying;

impl Applying {
    fn enter() -> Applying {
        APPLYING.with(|depth| depth.set(depth.get() + 1));
        Applying
    }
}

impl Drop for Applying {
    fn drop(&mut self) {
        APPLYING.with(|depth| depth.set(depth.get() - 1));
    }
}

/// What a rule does to the children of a parent row that is updated or deleted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Cascade,
    Restrict,
    SetNull,
    SetDefault,
}

impl Action {
    /// The action for a stored rule byte (`ADS_DD_RI_*`): 1 cascade, 2 restrict,
    /// 3 set null, 4 set default. Anything else is treated as RESTRICT, the
    /// safe default.
    fn from_u8(byte: u8) -> Action {
        match byte {
            1 => Action::Cascade,
            3 => Action::SetNull,
            4 => Action::SetDefault,
            _ => Action::Restrict,
        }
    }
}

/// One referential integrity rule as it bears on a table, with the column names
/// resolved so it can be enforced through SQL on the child table.
#[derive(Clone, Debug)]
struct Rule {
    name: String,
    child_table: String,
    /// The child's foreign-key column (its index expression).
    child_column: String,
    /// The child's foreign-key index tag, seeked to find children.
    child_index: String,
    parent_table: String,
    /// The parent's primary-key column (its index expression).
    parent_column: String,
    /// The parent's primary-key index tag, seeked to check a foreign key.
    parent_index: String,
    /// The child foreign-key column's declared default, for SET DEFAULT.
    child_default: Option<String>,
    /// The message the rule gives a child with no parent (its `NO_PKEY_ERROR`).
    no_parent: String,
    /// The message the rule gives a refused update or delete of a parent
    /// (its `CASCADE_ERROR`).
    cascade_error: String,
    update: Action,
    delete: Action,
}

/// The rules of a connection's dictionary that bear on one table: those where
/// it is the child (checked on its own insert and update) and those where it is
/// the parent (applied on its own key update and delete). Empty when the table
/// is in no rule, in a folder (no dictionary), or in a dictionary that cannot
/// be read — enforcement is then a no-op.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    as_child: Vec<Rule>,
    as_parent: Vec<Rule>,
}

impl Rules {
    pub fn is_empty(&self) -> bool {
        self.as_child.is_empty() && self.as_parent.is_empty()
    }
}

/// The rules of `connection`'s dictionary that bear on the table `table_name`.
pub fn load(session: &Session, connection: Handle, table_name: &str) -> Rules {
    let short = table_name.rsplit(['\\', '/']).next().unwrap_or(table_name);
    let short = short.split('.').next().unwrap_or(short);
    let Ok(owner) = connection::get(session, connection) else {
        return Rules::default();
    };
    owner
        .with_database_opt(|db| {
            let dict = db.dictionary();
            let Some(table) = dict.named(Kind::Table, short) else {
                return Rules::default();
            };
            // The index expression of a relation's index object, which is the
            // column its key is built from; `None` for a compound key, which
            // this cannot enforce through a single column.
            let column = |id: Option<u32>| -> Option<String> {
                let object = dict.object(id?)?;
                let expression = object.text(tflip_storage::dd::index::EXPRESSION)?;
                (!expression.contains(';') && !expression.contains('+'))
                    .then(|| expression.trim().to_owned())
            };
            let name_of = |id: Option<u32>| dict.object(id?).map(|o| o.name.clone());
            let mut rules = Rules::default();
            for object in dict.all(Kind::Relation) {
                let parent_table = name_of(object.u32(relation::PRIMARY_TABLE));
                let child_table = name_of(object.u32(relation::FOREIGN_TABLE));
                let parent_index = name_of(object.u32(relation::PRIMARY_INDEX));
                let child_index = name_of(object.u32(relation::FOREIGN_INDEX));
                let parent_column = column(object.u32(relation::PRIMARY_INDEX));
                let child_column = column(object.u32(relation::FOREIGN_INDEX));
                let (
                    Some(parent_table),
                    Some(child_table),
                    Some(parent_index),
                    Some(child_index),
                    Some(parent_column),
                    Some(child_column),
                ) = (
                    parent_table,
                    child_table,
                    parent_index,
                    child_index,
                    parent_column,
                    child_column,
                )
                else {
                    continue;
                };
                // The child foreign-key column's declared default (for SET
                // DEFAULT), from its field object.
                let child_default = object.u32(relation::FOREIGN_TABLE).and_then(|child_id| {
                    dict.children(child_id, Kind::Field)
                        .into_iter()
                        .find(|f| f.name.eq_ignore_ascii_case(&child_column))
                        .and_then(|f| f.text(tflip_storage::dd::field::DEFAULT_VALUE))
                        .filter(|d| !d.is_empty())
                });
                let rule = Rule {
                    name: object.name.clone(),
                    child_table,
                    child_column,
                    child_index,
                    parent_table,
                    parent_column,
                    parent_index,
                    child_default,
                    no_parent: object.text(relation::NO_PKEY_ERROR).unwrap_or_default(),
                    cascade_error: object.text(relation::CASCADE_ERROR).unwrap_or_default(),
                    update: Action::from_u8(object.u8(relation::UPDATE_RULE).unwrap_or(2)),
                    delete: Action::from_u8(object.u8(relation::DELETE_RULE).unwrap_or(2)),
                };
                if object.u32(relation::FOREIGN_TABLE) == Some(table.id) {
                    rules.as_child.push(rule.clone());
                }
                if object.u32(relation::PRIMARY_TABLE) == Some(table.id) {
                    rules.as_parent.push(rule);
                }
            }
            rules
        })
        .unwrap_or_default()
}

/// The value of the column `name` in a record of `fields`/`values`.
fn column_value<'a>(fields: &[Field], values: &'a [V], name: &str) -> Option<&'a V> {
    let index = fields
        .iter()
        .position(|f| f.name.eq_ignore_ascii_case(name))?;
    values.get(index)
}

/// A value as an SQL literal for a `WHERE`/`SET`, or `None` for a type a rule's
/// key is not expected to be (binary, GUID, time) — enforcement then skips it.
fn literal(value: &V) -> Option<String> {
    Some(match value {
        V::Null | V::Blank(_) => "NULL".to_owned(),
        V::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
        V::Int(n) => n.to_string(),
        V::Num(mantissa, scale) => decimal(*mantissa, *scale),
        V::Double(d) => format!("{d:?}"),
        V::Money(m) => decimal(i128::from(*m), 4),
        V::Str(s) => format!("'{}'", s.replace('\'', "''")),
        // A date or other key is rare and needs a typed literal; enforcement
        // skips it rather than risk a wrong one.
        V::Date(_) | V::Time(_) | V::TimeStamp(..) | V::Bin(_) | V::Guid(_) => return None,
    })
}

/// A scaled integer as a decimal string.
fn decimal(mantissa: i128, scale: u16) -> String {
    if scale == 0 {
        return mantissa.to_string();
    }
    let sign = if mantissa < 0 { "-" } else { "" };
    let digits = mantissa.unsigned_abs().to_string();
    let scale = usize::from(scale);
    let digits = format!("{digits:0>width$}", width = scale + 1);
    let point = digits.len() - scale;
    format!("{sign}{}.{}", &digits[..point], &digits[point..])
}

/// Checks the foreign keys of a record `values` just inserted into, or updated
/// in, a child table: each rule's non-NULL foreign key must have a parent.
pub fn check_child(
    session: &Session,
    connection: Handle,
    rules: &Rules,
    fields: &[Field],
    values: &[V],
) -> Result<(), Error> {
    for rule in &rules.as_child {
        let Some(key) = column_value(fields, values, &rule.child_column) else {
            continue;
        };
        if key.is_null() {
            continue;
        }
        if !exists(
            session,
            connection,
            &rule.parent_table,
            &rule.parent_index,
            key,
        )? {
            // A rule with no message of its own gets the engine's.
            let message = match rule.no_parent.is_empty() {
                true => format!(
                    "No primary key found in {}:{} for the foreign key update to {}:{}",
                    rule.parent_table, rule.parent_index, rule.child_table, rule.child_index
                ),
                false => rule.no_parent.clone(),
            };
            return Err(Error::NoParentKey(message));
        }
    }
    Ok(())
}

/// Applies the rules of a parent table whose row is being deleted: the old key
/// is `values`.
pub fn on_delete(
    session: &Session,
    connection: Handle,
    rules: &Rules,
    fields: &[Field],
    values: &[V],
    record: u32,
) -> Result<(), Error> {
    for rule in &rules.as_parent {
        apply(
            session,
            connection,
            rule,
            rule.delete,
            fields,
            values,
            None,
            record,
        )?;
    }
    Ok(())
}

/// Checks the RESTRICT rules of a parent table whose row is being updated from
/// `old` to `new`: a key column that changed while a child points at the old key
/// is refused. Run before the row is written.
pub fn before_update(
    session: &Session,
    connection: Handle,
    rules: &Rules,
    fields: &[Field],
    old: &[V],
    new: &[V],
    record: u32,
) -> Result<(), Error> {
    for rule in &rules.as_parent {
        if rule.update != Action::Restrict || !key_changed(rule, fields, old, new) {
            continue;
        }
        apply(
            session,
            connection,
            rule,
            Action::Restrict,
            fields,
            old,
            None,
            record,
        )?;
    }
    Ok(())
}

/// Applies the CASCADE / SET NULL / SET DEFAULT rules of a parent table whose
/// row was just updated from `old` to `new`, re-pointing or clearing the
/// children that held the old key. Run after the row is written, so the new key
/// a cascade re-points to already exists.
pub fn after_update(
    session: &Session,
    connection: Handle,
    rules: &Rules,
    fields: &[Field],
    old: &[V],
    new: &[V],
) -> Result<(), Error> {
    for rule in &rules.as_parent {
        if rule.update == Action::Restrict || !key_changed(rule, fields, old, new) {
            continue;
        }
        let after = column_value(fields, new, &rule.parent_column);
        apply(
            session,
            connection,
            rule,
            rule.update,
            fields,
            old,
            after,
            0,
        )?;
    }
    Ok(())
}

/// Whether a rule's parent key column differs between `old` and `new`.
fn key_changed(rule: &Rule, fields: &[Field], old: &[V], new: &[V]) -> bool {
    column_value(fields, old, &rule.parent_column) != column_value(fields, new, &rule.parent_column)
}

/// Applies `action` to the children that point at the old key of `rule` (read
/// from `old`); `new_key` is the parent's new key value on an update (`None` on
/// a delete); `record` is the number of the parent's record.
#[allow(clippy::too_many_arguments)]
fn apply(
    session: &Session,
    connection: Handle,
    rule: &Rule,
    action: Action,
    fields: &[Field],
    old: &[V],
    new_key: Option<&V>,
    record: u32,
) -> Result<(), Error> {
    let Some(key_value) = column_value(fields, old, &rule.parent_column) else {
        return Ok(());
    };
    if key_value.is_null() {
        return Ok(());
    }
    if action == Action::Restrict {
        // A child still pointing at the key refuses the update or delete.
        return match exists(
            session,
            connection,
            &rule.child_table,
            &rule.child_index,
            key_value,
        )? {
            // A rule with no message of its own gets the engine's, which names
            // the rule, the tables and the parent's record
            // (`fixtures/sql/spddri2.json`).
            true => Err(Error::RiRestrict(match rule.cascade_error.is_empty() {
                true => format!(
                    "The restrict RI rule is named \"{}\".  It links the parent table \"{}\" to \
                     the child table \"{}\".  The record number in the parent table where the \
                     problem occurs is {record}",
                    rule.name, rule.parent_table, rule.child_table
                ),
                false => rule.cascade_error.clone(),
            })),
            false => Ok(()),
        };
    }
    let _applying = Applying::enter();
    // The children the transaction wrote cannot be locked by the rule: it
    // fails before changing any.
    if let Some(error) = locked_child(session, connection, rule, key_value)? {
        return Err(error);
    }
    let to = match action {
        // A cascaded delete runs as SQL (a child DELETE), which does not
        // create a client-visible cursor the way a SELECT would.
        Action::Cascade if new_key.is_none() => {
            let Some(key) = literal(key_value) else {
                return Ok(());
            };
            let (child, column) = (&rule.child_table, &rule.child_column);
            let sql = format!("DELETE FROM [{child}] WHERE [{child}].[{column}] = {key}");
            return exec(session, connection, &sql);
        }
        Action::Cascade => match new_key.and_then(set_value) {
            Some(to) => to,
            None => return Ok(()),
        },
        Action::SetNull => SetValue::Null,
        // The column's declared default (NULL if none); the default must then
        // itself be a valid parent key, so a default with no parent row makes
        // the write fail, as the original does.
        Action::SetDefault => match &rule.child_default {
            Some(text) => SetValue::Field(text.clone()),
            None => SetValue::Null,
        },
        Action::Restrict => unreachable!(),
    };
    update_children(session, connection, rule, key_value, &to)
}

/// The 5035 of a rule that cannot lock a child the connection's transaction
/// wrote, if one of the children of `key` is that (`fixtures/sql/txtrig.json`).
fn locked_child(
    session: &Session,
    connection: Handle,
    rule: &Rule,
    key: &V,
) -> Result<Option<Error>, Error> {
    let owner = connection::get(session, connection).ok();
    let find = || -> Result<Option<Error>, Error> {
        let handle = table::open(
            session,
            connection,
            &rule.child_table,
            TableSettings::default(),
        )?;
        let found = (|| {
            let child = table::get(session, handle)?;
            let order = index::get(session, index::by_name(session, handle, &rule.child_index)?)?;
            let Some(seek) = seek_value(key) else {
                return Ok(None);
            };
            if !child.seek_in(&order, seek, SeekType::Hard)? {
                return Ok(None);
            }
            while !child.position().eof {
                let Some((fields, values)) = child.ri_current_row()? else {
                    break;
                };
                let held = column_value(&fields, &values, &rule.child_column);
                if held.and_then(literal) != literal(key) {
                    break;
                }
                let record = child.record_number();
                if child.written_by_transaction(record) {
                    return Ok(Some(Error::Sql(SqlError::ace(
                        5035,
                        &format!(
                            "The requested lock could not be granted.  The file or record may be \
                             locked by another user.  This record lock was necessary because of a \
                             cascaded update or delete due to exising referential integrity rules.  \
                             Table: {}  Record #: {record}",
                            child.path().display()
                        ),
                        "",
                    ))));
                }
                child.skip_in(&order, 1)?;
            }
            Ok(None)
        })();
        let _ = table::close(session, handle);
        found
    };
    match &owner {
        Some(owner) => owner.as_owner(find),
        None => find(),
    }
}

/// Whether the index `tag` of the table `name` has the key `value`, sought as
/// the dictionary owner so access rights do not refuse the read (like a trigger
/// body). A key type a seek cannot build is treated as present, so enforcement
/// does not wrongly refuse it.
fn exists(
    session: &Session,
    connection: Handle,
    name: &str,
    tag: &str,
    value: &V,
) -> Result<bool, Error> {
    let owner = connection::get(session, connection).ok();
    let seek = || -> Result<bool, Error> {
        let handle = table::open(session, connection, name, TableSettings::default())?;
        let found = (|| {
            let order = index::get(session, index::by_name(session, handle, tag)?)?;
            let Some(seek) = seek_value(value) else {
                return Ok(true);
            };
            table::get(session, handle)?.seek_in(&order, seek, SeekType::Hard)
        })();
        let _ = table::close(session, handle);
        found
    };
    match &owner {
        Some(owner) => owner.as_owner(seek),
        None => seek(),
    }
}

/// The number of the first record of `child_table` that a rule would refuse:
/// one not deleted whose foreign key `child_column` is not NULL and has no
/// parent in `parent_index` of `parent_table` (`fixtures/sql/spddri2.json`).
pub fn first_orphan(
    session: &Session,
    connection: Handle,
    parent_table: &str,
    parent_index: &str,
    child_table: &str,
    child_column: &str,
) -> Result<Option<u32>, Error> {
    let handle = table::open(session, connection, child_table, TableSettings::default())?;
    let found = (|| {
        let child = table::get(session, handle)?;
        child.goto_top()?;
        while !child.position().eof {
            let record = child.record_number();
            if let Some((fields, values)) = child.ri_current_row()?
                && let Some(key) = column_value(&fields, &values, child_column)
                && !key.is_null()
                && !exists(session, connection, parent_table, parent_index, key)?
            {
                return Ok(Some(record));
            }
            child.skip(1)?;
        }
        Ok(None)
    })();
    let _ = table::close(session, handle);
    found
}

/// The key a seek finds `value` by, or `None` for a type a rule's key is not
/// expected to be.
fn seek_value(value: &V) -> Option<SeekValue<'_>> {
    Some(match value {
        V::Int(n) => SeekValue::Double(*n as f64),
        V::Num(m, s) => SeekValue::Double(*m as f64 / 10f64.powi(i32::from(*s))),
        V::Double(d) => SeekValue::Double(*d),
        V::Money(m) => SeekValue::Double(*m as f64 / 10_000.0),
        V::Date(j) => SeekValue::Double(f64::from(*j)),
        V::Str(s) => SeekValue::Text(s),
        V::Null | V::Blank(_) | V::Bool(_) | V::Time(_) | V::TimeStamp(..) | V::Bin(_) => {
            return None;
        }
        V::Guid(_) => return None,
    })
}

/// What a child's foreign key is set to for `value`, a parent's key.
fn set_value(value: &V) -> Option<SetValue> {
    Some(match value {
        V::Null => SetValue::Null,
        V::Int(n) => SetValue::LongLong(*n),
        V::Num(m, s) => SetValue::Field(decimal(*m, *s)),
        V::Double(d) => SetValue::Double(*d),
        V::Money(m) => SetValue::Money(*m),
        V::Date(j) => SetValue::Julian(*j),
        V::Str(s) => SetValue::String(s.clone()),
        _ => return None,
    })
}

/// Sets the foreign key of each child of `rule` that holds `key` to `to`,
/// through the table API, as the original engine re-points them (probed,
/// `tools/ace/dd/rules.ops`): a child the set's constraint refuses is still
/// written, unchanged (its triggers firing), and the refusal ends the
/// change. Run as the dictionary owner, like [`exec`].
fn update_children(
    session: &Session,
    connection: Handle,
    rule: &Rule,
    key: &V,
    to: &SetValue,
) -> Result<(), Error> {
    let owner = connection::get(session, connection).ok();
    let run = || -> Result<(), Error> {
        let handle = table::open(
            session,
            connection,
            &rule.child_table,
            TableSettings::default(),
        )?;
        let result = (|| {
            let child = table::get(session, handle)?;
            let order = index::get(session, index::by_name(session, handle, &rule.child_index)?)?;
            // The children first, as setting their keys moves them in the order.
            let mut records = Vec::new();
            if let Some(seek) = seek_value(key)
                && child.seek_in(&order, seek, SeekType::Hard)?
            {
                while !child.position().eof {
                    let Some((fields, values)) = child.ri_current_row()? else {
                        break;
                    };
                    let held = column_value(&fields, &values, &rule.child_column);
                    if held.and_then(literal) != literal(key) {
                        break;
                    }
                    records.push(child.record_number());
                    child.skip_in(&order, 1)?;
                }
            }
            for record in records {
                child.goto_record(record)?;
                let set = child.set_field(table::FieldRef::Name(&rule.child_column), to);
                // The set's refusal is what is reported, over what writing
                // the unchanged record (whose old key has no parent now) met.
                if let Err(error) = crate::trigger::write_record_api(session, &child) {
                    let _ = child.cancel_update();
                    return set.and(Err(error));
                }
                set?;
            }
            Ok(())
        })();
        let _ = table::close(session, handle);
        result
    };
    match &owner {
        Some(owner) => owner.as_owner(run),
        None => run(),
    }
}

/// Puts the record `slot` of `target`, a parent whose rules failed to apply
/// to its children after it was written, back as `old` held it, as the
/// original engine leaves it (probed, `tools/ace/dd/rules.ops`); its memo
/// fields keep what the write left in them.
pub fn restore(target: &table::Table, slot: u32, mut old: Vec<u8>) -> Result<(), Error> {
    target.goto_record(slot)?;
    if let Some(now) = target.stored_bytes()? {
        for field in target.fields().iter().filter(|f| f.is_memo()) {
            let start = usize::from(field.offset);
            let range = start..start + usize::from(field.length());
            old[range.clone()].copy_from_slice(&now[range]);
        }
    }
    target.set_record(&old)?;
    target.write_record()
}

/// Runs the DML statement `sql` on a fresh statement of `connection`, as the
/// dictionary owner (so the connected user's access rights do not refuse a
/// cascade, like a trigger body); the statement is closed after. The statement's
/// own implicit transaction joins the caller's, so a cascade that fails rolls
/// back with the statement that drove it.
fn exec(session: &Session, connection: Handle, sql: &str) -> Result<(), Error> {
    let owner = connection::get(session, connection).ok();
    let run = || -> Result<(), Error> {
        let statement = statement::create(session, connection)?;
        let result = statement::execute_direct(session, statement, sql, false).map(drop);
        let _ = statement::close(session, statement);
        result
    };
    match &owner {
        Some(owner) => owner.as_owner(run),
        None => run(),
    }
}
