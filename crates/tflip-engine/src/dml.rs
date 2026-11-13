//! `INSERT`, `UPDATE` and `DELETE`: the records a statement changes, written
//! as a client's calls would write them (`AdsAppendRecord`, `AdsSet*`,
//! `AdsWriteRecord`, `AdsDeleteRecord`), so that locks, AutoInc values,
//! memo values and index orders follow the same rules.
//!
//! The rows to change and their new values are worked out first, the
//! writes done after: a statement never sees its own changes. It stops at
//! the first error, the rows written before it staying written; but an
//! `UPDATE` or `DELETE` passes by a record it cannot lock and goes on,
//! failing with the lock's error at the end, as the help says of `DELETE`
//! (`master_delete`) and the original engine does (`fixtures/write/tx_sql`).
//! In a transaction of the statement's own (`SET TRANSACTION`), what it
//! wrote is committed or rolled back after (see [`crate::transaction`]).

use std::cell::Cell;
use std::sync::Arc;

use tflip_sql::ast::Mark;
use tflip_sql::dml::{Assign, Dml, DmlKind, DmlRun};
use tflip_sql::{SqlError, V};
use tflip_storage::field::{Field, FieldType};
use tflip_storage::memo::MemoFile;
use tflip_storage::value::Value;

use crate::filter::SqlRow;
use crate::handle::Handle;
use crate::setters::SetValue;
use crate::statement::Settings;
use crate::table::{self, FieldRef};
use crate::trigger::{self, Affected, Record, Set, Timing};
use crate::{Error, Session, ri};

/// The context a statement's triggers fire in: the connection it runs on,
/// the table's triggers for the event, and whether the record being changed
/// is in a transaction of its own (see [`Fire::record`]).
struct Fire<'a> {
    session: &'a Session,
    connection: Handle,
    settings: &'a Settings,
    set: Set,
    in_transaction: Cell<bool>,
}

impl Fire<'_> {
    /// Fires the triggers of `timing` on one affected record.
    fn at(
        &self,
        timing: Timing,
        new: Option<&Record>,
        old: Option<&Record>,
        recno: u32,
    ) -> Result<(), Error> {
        let transaction = self.in_transaction.get();
        let at = Affected {
            new,
            old,
            recno,
            transaction,
        };
        self.set
            .fire(self.session, self.connection, self.settings, timing, &at)
    }

    /// Runs `change`, one record's change and the triggers it fires: with
    /// real transactions, in a transaction of its own that a failure takes
    /// back, unless the event has no triggers or a `NO TRANSACTION` one (as
    /// the original's server, `fixtures/sql/triggers.json`, `txtrig.json`).
    fn record<T>(&self, change: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        if self.set.is_empty() {
            return change();
        }
        // The triggers of an event with a `NO TRANSACTION` one run in the
        // caller's transaction, but as a call is: it may not end the caller's
        // levels, nor roll back to its savepoints.
        if !self.set.transactional() {
            crate::transaction::call_begins(self.session, self.connection)?;
            let result = change();
            let ended =
                crate::transaction::call_ends(self.session, self.connection, result.is_ok());
            return result.and_then(|value| ended.map(|()| value));
        }
        let Some(nested) = crate::transaction::trigger_begins(self.session, self.connection)?
        else {
            return change();
        };
        self.in_transaction.set(true);
        let result = change();
        self.in_transaction.set(false);
        let ended = crate::transaction::trigger_ends(self.session, nested, result.is_ok());
        result.and_then(|value| ended.map(|()| value))
    }
}

/// What running a statement did: the rows it changed, and how it ended.
/// No rows means it failed before changing any (`u32::MAX` if before it
/// ran, as the original engine counts).
pub(crate) struct Done {
    pub rows: u32,
    pub result: Result<(), Error>,
}

/// The columns a statement writes and its table, as the functions that
/// write a record need them.
struct Columns<'a> {
    assigns: &'a [Assign],
    table: &'a str,
}

/// Runs `dml` on the table `name` of `connection`, its expressions through
/// `run`.
pub(crate) fn run(
    session: &Session,
    connection: Handle,
    settings: &crate::statement::Settings,
    name: &str,
    dml: &Dml,
    run: &DmlRun<'_>,
) -> Done {
    // The statement's own open of its tables was the one the triggers saw.
    let opened = crate::trigger::unseen(|| {
        table::open(session, connection, name, settings.table_named(0, name))
    });
    let handle = match opened {
        Ok(handle) => handle,
        Err(error) => {
            let error = crate::statement::open_error(error, name);
            return Done {
                rows: u32::MAX,
                result: Err(error),
            };
        }
    };
    let fire = |kind| Fire {
        session,
        connection,
        settings,
        set: trigger::load(session, connection, name, kind).with_rowid_prefix(
            table::get(session, handle)
                .ok()
                .and_then(|target| target.sql_info().rowid_prefix),
        ),
        in_transaction: Cell::new(false),
    };
    let done = match table::get(session, handle) {
        // As the original engine does, a `DELETE` the user may not do fails
        // as it opens the table (probed, `fixtures/sql/links.json`).
        Ok(target)
            if match dml.kind {
                DmlKind::Delete => target.access & crate::system::right::DELETE == 0,
                DmlKind::Insert => target.access & crate::system::right::INSERT == 0,
                DmlKind::Update => target.access & crate::system::right::UPDATE == 0,
                _ => false,
            } =>
        {
            let error = SqlError::ace(
                7087,
                &format!(
                    "Cannot open table or view for this operation because the requested \
                     permissions have not been granted to the user. Table name: {name}"
                ),
                "",
            );
            Done {
                rows: u32::MAX,
                result: Err(Error::Sql(error)),
            }
        }
        Ok(target) => {
            let rules = &target.ri;
            let mut rows = 0;
            let cols = Columns {
                assigns: &dml.assigns,
                table: &dml.table_name,
            };
            let result = match dml.kind {
                DmlKind::Insert => insert(
                    &fire(DmlKind::Insert),
                    rules,
                    &target,
                    &cols,
                    &dml.defaults,
                    run,
                    &mut rows,
                ),
                DmlKind::Update if dml.from.is_some() => update_from(
                    &fire(DmlKind::Update),
                    rules,
                    &target,
                    &cols,
                    dml,
                    run,
                    &mut rows,
                ),
                DmlKind::Update => update(
                    &fire(DmlKind::Update),
                    rules,
                    &target,
                    &cols,
                    dml.mark,
                    run,
                    &mut rows,
                ),
                DmlKind::Delete => delete(&fire(DmlKind::Delete), rules, &target, run, &mut rows),
                DmlKind::Merge => {
                    let fires = (fire(DmlKind::Insert), fire(DmlKind::Update));
                    merge(&fires, rules, &target, dml, run, &mut rows)
                }
            };
            Done { rows, result }
        }
        Err(error) => Done {
            rows: u32::MAX,
            result: Err(error),
        },
    };
    let _ = table::close(session, handle);
    done
}

/// The table's records that may hold for the statement's `WHERE`, in
/// record order, each its slot and its fields' values: those the table's
/// orders find for its comparisons (as for a live cursor), else all.
fn records(target: &Arc<table::Table>, run: &DmlRun<'_>) -> Result<Vec<(u32, Vec<V>)>, Error> {
    let fields = target.fields().to_vec();
    let mut out = Vec::new();
    fn values(
        fields: &[Field],
        slot: u32,
        bytes: &[u8],
        memo: Option<&mut MemoFile>,
    ) -> Result<Vec<V>, Error> {
        let mut row = SqlRow {
            fields,
            bytes,
            memo,
            record: Some(slot),
            hidden: &[],
        };
        tflip_sql::dml::row_values(fields, &mut row).map_err(Error::Sql)
    }
    match candidates(target, run)? {
        Some(slots) => {
            for slot in slots {
                target.read_active(slot, |bytes, memo| {
                    out.push((slot, values(&fields, slot, bytes, memo)?));
                    Ok(())
                })?;
            }
        }
        None => target.scan(|slot, bytes, memo| {
            out.push((slot, values(&fields, slot, bytes, memo)?));
            Ok(())
        })?,
    }
    Ok(out)
}

/// The records the orders find for the comparisons of the statement's
/// `WHERE` written as the original engine's AOF, in record order; `None`
/// where they cannot tell.
fn candidates(target: &Arc<table::Table>, run: &DmlRun<'_>) -> Result<Option<Vec<u32>>, Error> {
    let Some(filter) = run.filter() else {
        return Ok(None);
    };
    let Some(text) = tflip_sql::xbase::aof(filter, target.fields(), run.params()) else {
        return Ok(None);
    };
    let orders = crate::index::private_orders(target, true);
    let Ok((optimized, serving)) = target.optimize_with(&text, &orders, &[]) else {
        return Ok(None);
    };
    let found = target.lookup_superset(&optimized.bound, &serving)?;
    Ok(found.map(|found| found.records().collect()))
}

fn insert(
    fire: &Fire,
    rules: &ri::Rules,
    target: &table::Table,
    cols: &Columns,
    defaults: &[Vec<bool>],
    run: &DmlRun<'_>,
    rows: &mut u32,
) -> Result<(), Error> {
    let values = run.rows().map_err(Error::Sql)?;
    for (n, row) in values.into_iter().enumerate() {
        let (kept, row) = given(cols, defaults.get(n), row);
        let cols = &Columns {
            assigns: kept.as_deref().unwrap_or(cols.assigns),
            table: cols.table,
        };
        // Values read before the record is appended.
        let row = read(cols, run, row)?;
        let new = new_record(target, cols, &row);
        let truncated = fire
            .record(|| insert_one(fire, rules, target, cols, run, row, &new, Mark::Keep, rows))?;
        if let Some(error) = truncated {
            return Err(Error::Sql(error));
        }
    }
    Ok(())
}

/// `row` without its `DEFAULT` values (`flags` marks them), which leave
/// their columns as a new record has them, and the columns that remain
/// (`None` if all do).
fn given(
    cols: &Columns,
    flags: Option<&Vec<bool>>,
    row: Vec<(V, usize)>,
) -> (Option<Vec<Assign>>, Vec<(V, usize)>) {
    let Some(flags) = flags.filter(|flags| flags.contains(&true)) else {
        return (None, row);
    };
    let kept = |i: &usize| !flags.get(*i).copied().unwrap_or(false);
    let assigns = (0..cols.assigns.len()).filter(kept);
    let assigns = assigns.map(|i| cols.assigns[i].clone()).collect();
    let row = row
        .into_iter()
        .enumerate()
        .filter(|(i, _)| kept(i))
        .map(|(_, value)| value)
        .collect();
    (Some(assigns), row)
}

/// Inserts one record, `row` (`new` as the triggers read it), firing its
/// triggers; the text cut to fit is the error for after it.
#[allow(clippy::too_many_arguments)]
fn insert_one(
    fire: &Fire,
    rules: &ri::Rules,
    target: &table::Table,
    cols: &Columns,
    run: &DmlRun<'_>,
    row: Vec<(V, usize)>,
    new: &Record,
    mark: Mark,
    rows: &mut u32,
) -> Result<Option<SqlError>, Error> {
    // An INSTEAD OF trigger replaces the insert, once the record has passed
    // the dictionary's rules (`fixtures/sql/trignn.json`).
    if fire.set.has_instead_of() {
        // Its record number is the slot the insert would take.
        let recno = target.next_insert_slot();
        refuse_unfit(target, cols, run, row, true)?;
        fire.at(Timing::InsteadOf, Some(new), None, recno)?;
        *rows += 1;
        return Ok(None);
    }
    // The record is appended (with the dictionary's defaults) and its
    // columns set, each checked against its constraints as it is set; then
    // the record is checked as a whole, and only then do the BEFORE
    // triggers fire (probed, `tools/ace/dd/constraints.c`).
    target.append()?;
    let recno = target.record_number();
    let written = set_all(target, cols, run, row).and_then(|truncated| {
        target.check_pending()?;
        let new = written_record(target, new);
        fire.at(Timing::Before, Some(&new), None, recno)?;
        // A foreign key must have its parent before the row is written.
        ri::check_child(
            fire.session,
            fire.connection,
            rules,
            new.fields(),
            new.values(),
        )?;
        mark_record(target, mark)?;
        write(target, cols)?;
        remember(fire, target, recno, true);
        Ok((truncated, new))
    });
    let (truncated, new) = match written {
        Ok(written) => written,
        Err(error) => {
            // A DBF record cannot be taken back: the original engine
            // writes the one a failed `INSERT` appended as it is (its
            // AutoInc value too, `fixtures/write/sql_dbf`), deleted, but
            // for the one an order refuses the key of, which stays blank
            // (`fixtures/sql/dbfpk`).
            let kept = target.dbf_flavour().is_some()
                && !is_key_refused(&error)
                && target
                    .delete_record()
                    .and_then(|()| target.write_record())
                    .is_ok();
            if !kept {
                // An ADT record a dictionary's rules refused keeps its values
                // in its deleted slot (`fixtures/sql/ddtables`).
                let _ = match error {
                    Error::Constraint { .. } => target.cancel_refused(),
                    _ => target.cancel_update(),
                };
            }
            return Err(error);
        }
    };
    if !fire.set.is_empty() {
        crate::transaction::insert_fires(fire.session, fire.connection, target.shared(), recno);
    }
    if let Err(error) = fire.at(Timing::After, Some(&new), None, recno) {
        // The record a failing AFTER trigger fired on goes, with the
        // trigger's transaction or (`NO TRANSACTION`, or no transactions)
        // deleted, as in the original engine (`fixtures/sql/triggers.json`).
        if !fire.in_transaction.get() {
            let _ = target.delete_record().and_then(|()| target.write_record());
        }
        return Err(error);
    }
    *rows += 1;
    Ok(truncated)
}

fn update(
    fire: &Fire,
    rules: &ri::Rules,
    target: &Arc<table::Table>,
    cols: &Columns,
    mark: Mark,
    run: &DmlRun<'_>,
    rows: &mut u32,
) -> Result<(), Error> {
    // The rows that change, their old values and their new values, before any
    // changes.
    let mut changes = Vec::new();
    for (slot, values) in records(target, run)? {
        if run.matches(slot, &values).map_err(Error::Sql)? {
            let sets = run.sets(slot, &values).map_err(Error::Sql)?;
            let sets = read(cols, run, sets)?;
            let old = Record::new(target.fields().to_vec(), values.clone());
            let new = updated_record(target, cols, &values, &sets);
            changes.push((slot, old, new, sets));
        }
    }
    let (mut locked, mut truncated) = (false, None);
    for (slot, old, new, sets) in changes {
        target.goto_record(slot)?;
        let done = fire.record(|| {
            let args = (slot, &old, &new);
            update_one(fire, rules, target, cols, run, args, sets, mark, rows)
        });
        match done {
            Ok(cut) => truncated = truncated.or(cut),
            Err(Error::LockFailed) => locked = true,
            Err(error) => return Err(error),
        }
    }
    if let Some(error) = truncated {
        return Err(Error::Sql(error));
    }
    lock_error(locked)
}

/// Runs an `UPDATE … FROM`: each record of the table, in the order the join
/// reads them, is updated from the first row of the join it is in, and sees
/// what the records before it wrote if the join reads the table again
/// (`fixtures/sql/updfrom.json`). Text cut to fit is an error for the end of
/// the statement.
fn update_from(
    fire: &Fire,
    rules: &ri::Rules,
    target: &Arc<table::Table>,
    cols: &Columns,
    dml: &Dml,
    run: &DmlRun<'_>,
    rows: &mut u32,
) -> Result<(), Error> {
    let parts = dml.from.as_ref().expect("a FROM");
    let (mut truncated, mut locked) = (None, false);
    let mut updated = std::collections::HashSet::new();
    let mut joined = run.from_rows().map_err(Error::Sql)?;
    let mut next = 0;
    while let Some(row) = joined.get(next) {
        next += 1;
        // A record the join has no row of (an outer join's) is not updated;
        // each is updated once.
        let slot = match row.last() {
            Some(V::Int(slot)) if *slot > 0 => *slot as u32,
            _ => continue,
        };
        if !updated.insert(slot) {
            continue;
        }
        let values = row[parts.start..parts.start + parts.width].to_vec();
        let sets = run.sets(slot, row).map_err(Error::Sql)?;
        let sets = read(cols, run, sets)?;
        let old = Record::new(target.fields().to_vec(), values.clone());
        let new = updated_record(target, cols, &values, &sets);
        target.goto_record(slot)?;
        let written = *rows;
        let result = fire.record(|| {
            let args = (slot, &old, &new);
            update_one(fire, rules, target, cols, run, args, sets, dml.mark, rows)
        });
        match result {
            Ok(cut) => truncated = truncated.or(cut),
            Err(Error::LockFailed) => locked = true,
            Err(error) => return Err(error),
        }
        if *rows != written {
            run.refresh();
            if parts.live {
                joined = run.from_rows().map_err(Error::Sql)?;
                next = 0;
            }
        }
    }
    if let Some(error) = truncated {
        return Err(Error::Sql(error));
    }
    lock_error(locked)
}

/// Updates the record in `slot` from `old` to `new` (`sets` the values
/// set), firing its triggers; the text cut to fit is the error for after it.
#[allow(clippy::too_many_arguments)]
fn update_one(
    fire: &Fire,
    rules: &ri::Rules,
    target: &Arc<table::Table>,
    cols: &Columns,
    run: &DmlRun<'_>,
    (slot, old, new): (u32, &Record, &Record),
    sets: Vec<(V, usize)>,
    mark: Mark,
    rows: &mut u32,
) -> Result<Option<SqlError>, Error> {
    // An INSTEAD OF trigger replaces the update, once the record has passed
    // the dictionary's rules (`fixtures/sql/trignn.json`).
    if fire.set.has_instead_of() {
        refuse_unfit(target, cols, run, sets, false)?;
        fire.at(Timing::InsteadOf, Some(new), Some(old), slot)?;
        *rows += 1;
        return Ok(None);
    }
    let written = set_all(target, cols, run, sets).and_then(|truncated| {
        // The record's constraints are checked before the BEFORE triggers.
        target.check_pending()?;
        fire.at(Timing::Before, Some(new), Some(old), slot)?;
        // Before the write: the new foreign keys must have their parents,
        // and a RESTRICT rule refuses a changed primary key with children.
        ri::check_child(
            fire.session,
            fire.connection,
            rules,
            new.fields(),
            new.values(),
        )?;
        ri::before_update(
            fire.session,
            fire.connection,
            rules,
            old.fields(),
            old.values(),
            new.values(),
            slot,
        )?;
        let stored = match rules.is_empty() {
            true => None,
            false => target.stored_bytes()?,
        };
        mark_record(target, mark)?;
        write(target, cols)?;
        remember(fire, target, slot, false);
        // After the write (the new key now exists): cascade / set null /
        // set default re-point or clear this row's children; a child that
        // refuses puts the row back as it was.
        if let Err(error) = ri::after_update(
            fire.session,
            fire.connection,
            rules,
            old.fields(),
            old.values(),
            new.values(),
        ) {
            if let Some(stored) = stored {
                let _ = ri::restore(target, slot, stored);
            }
            return Err(error);
        }
        Ok(truncated)
    });
    match written {
        Ok(truncated) => {
            *rows += 1;
            fire.at(Timing::After, Some(new), Some(old), slot)?;
            Ok(truncated)
        }
        Err(error) => {
            let _ = target.cancel_update();
            Err(error)
        }
    }
}

/// Runs a `MERGE`: for each row of the source, the table's rows its
/// condition holds for are updated, or a row is inserted if there are none.
/// Each source row sees what those before it wrote, even if the source is
/// the table itself (`fixtures/sql/merge.json`); text cut to fit is an error
/// for the end of the statement.
fn merge(
    fires: &(Fire, Fire),
    rules: &ri::Rules,
    target: &Arc<table::Table>,
    dml: &Dml,
    run: &DmlRun<'_>,
    rows: &mut u32,
) -> Result<(), Error> {
    let parts = dml.merge.as_ref().expect("a MERGE");
    let (insert_fire, update_fire) = fires;
    let update_cols = Columns {
        assigns: &dml.assigns,
        table: &dml.table_name,
    };
    let insert_cols = parts.insert.as_ref().map(|insert| Columns {
        assigns: &insert.assigns,
        table: &dml.table_name,
    });
    let mut source = run.source_rows().map_err(Error::Sql)?;
    let (mut next, mut truncated, mut locked) = (0, None, false);
    while let Some(row) = source.get(next).cloned() {
        next += 1;
        let step = run;
        let written = *rows;
        let mut found = Vec::new();
        for (slot, values) in records(target, step)? {
            if step
                .merge_matches(slot, &values, &row)
                .map_err(Error::Sql)?
            {
                found.push((slot, values));
            }
        }
        match (found.is_empty(), &parts.insert, parts.update) {
            (true, Some(insert), _) => {
                let all = insert_cols.as_ref().expect("an INSERT's columns");
                let mark = match insert.deleted {
                    true => Mark::Delete,
                    false => Mark::Keep,
                };
                for (n, values) in step
                    .merge_rows(&row)
                    .map_err(Error::Sql)?
                    .into_iter()
                    .enumerate()
                {
                    let (kept, values) = given(all, insert.defaults.get(n), values);
                    let cols = &Columns {
                        assigns: kept.as_deref().unwrap_or(all.assigns),
                        table: all.table,
                    };
                    let values = read(cols, step, values)?;
                    let new = new_record(target, cols, &values);
                    let cut = insert_fire.record(|| {
                        insert_one(
                            insert_fire,
                            rules,
                            target,
                            cols,
                            step,
                            values,
                            &new,
                            mark,
                            rows,
                        )
                    })?;
                    truncated = truncated.or(cut);
                }
            }
            (false, _, Some(mark)) => {
                let mut changes = Vec::new();
                for (slot, values) in found {
                    let sets = step.merge_sets(slot, &values, &row).map_err(Error::Sql)?;
                    let sets = read(&update_cols, step, sets)?;
                    let old = Record::new(target.fields().to_vec(), values.clone());
                    let new = updated_record(target, &update_cols, &values, &sets);
                    changes.push((slot, old, new, sets));
                }
                for (slot, old, new, sets) in changes {
                    target.goto_record(slot)?;
                    let done = update_fire.record(|| {
                        let args = (slot, &old, &new);
                        update_one(
                            update_fire,
                            rules,
                            target,
                            &update_cols,
                            step,
                            args,
                            sets,
                            mark,
                            rows,
                        )
                    });
                    match done {
                        Ok(cut) => truncated = truncated.or(cut),
                        Err(Error::LockFailed) => locked = true,
                        Err(error) => return Err(error),
                    }
                }
            }
            _ => {}
        }
        if *rows != written {
            run.refresh();
            if parts.live_source {
                source = run.source_rows().map_err(Error::Sql)?;
            }
        }
    }
    if let Some(error) = truncated {
        return Err(Error::Sql(error));
    }
    lock_error(locked)
}

/// Marks the record being written as the `MERGE` clause says: a DBF
/// record can be a deleted one or be recalled.
fn mark_record(target: &table::Table, mark: Mark) -> Result<(), Error> {
    match mark {
        Mark::Keep => Ok(()),
        Mark::Delete => target.delete_record_keeping(),
        Mark::Recall => target.recall_record(),
    }
}

/// The error of a statement that passed by records it could not lock.
fn lock_error(locked: bool) -> Result<(), Error> {
    match locked {
        true => Err(Error::Sql(SqlError::ace(
            5035,
            "The requested lock could not be granted.  The file or record may be locked by \
             another user.",
            "",
        ))),
        false => Ok(()),
    }
}

fn delete(
    fire: &Fire,
    rules: &ri::Rules,
    target: &Arc<table::Table>,
    run: &DmlRun<'_>,
    rows: &mut u32,
) -> Result<(), Error> {
    let mut slots = Vec::new();
    for (slot, values) in records(target, run)? {
        if run.matches(slot, &values).map_err(Error::Sql)? {
            let old = Record::new(target.fields().to_vec(), values);
            slots.push((slot, old));
        }
    }
    let mut locked = false;
    for (slot, old) in slots {
        target.goto_record(slot)?;
        match fire.record(|| delete_one(fire, rules, target, slot, &old, rows)) {
            Ok(()) => {}
            Err(Error::LockFailed) => locked = true,
            Err(error) => return Err(error),
        }
    }
    lock_error(locked)
}

/// Deletes the record in `slot` (`old` as the triggers read it), firing its
/// triggers.
fn delete_one(
    fire: &Fire,
    rules: &ri::Rules,
    target: &Arc<table::Table>,
    slot: u32,
    old: &Record,
    rows: &mut u32,
) -> Result<(), Error> {
    // An INSTEAD OF trigger replaces the delete.
    if fire.set.has_instead_of() {
        fire.at(Timing::InsteadOf, None, Some(old), slot)?;
        *rows += 1;
        return Ok(());
    }
    fire.at(Timing::Before, None, Some(old), slot)?;
    // A rule applies to this parent row's children (cascade/set null, or
    // RESTRICT which refuses) before the row is deleted.
    ri::on_delete(
        fire.session,
        fire.connection,
        rules,
        old.fields(),
        old.values(),
        slot,
    )?;
    match target.delete_record().and_then(|()| target.write_record()) {
        Ok(()) => {
            *rows += 1;
            fire.at(Timing::After, None, Some(old), slot)
        }
        Err(Error::LockFailed) => {
            let _ = target.cancel_update();
            Err(Error::LockFailed)
        }
        Err(error) => Err(error),
    }
}

/// Checks `values` as the record an `INSTEAD OF` trigger stands in for (a new
/// one if `append`, else the current) against the dictionary's rules, as a
/// write would, and takes the record back.
fn refuse_unfit(
    target: &table::Table,
    cols: &Columns,
    run: &DmlRun<'_>,
    values: Vec<(V, usize)>,
    append: bool,
) -> Result<(), Error> {
    if append {
        // A DBF record cannot be taken back, and an INSTEAD OF trigger
        // would leave a deleted one for each insert (not probed).
        if target.dbf_flavour().is_some() {
            return Ok(());
        }
        target.append()?;
    }
    let checked = set_all(target, cols, run, values).and_then(|_| target.check_pending());
    match checked {
        Ok(()) => target.cancel_update(),
        Err(error) => {
            let _ = match error {
                Error::Constraint { .. } => target.cancel_refused(),
                _ => target.cancel_update(),
            };
            Err(error)
        }
    }
}

/// Sets the current record's columns to `values`, an assignment each (and
/// where each value is written, for errors). Text cut to fit is set, and
/// its error returned for after the record is written.
fn set_all(
    target: &table::Table,
    cols: &Columns,
    run: &DmlRun<'_>,
    values: Vec<(V, usize)>,
) -> Result<Option<SqlError>, Error> {
    let mut truncated = None;
    for (assign, (v, pos)) in cols.assigns.iter().zip(values) {
        if !assign.takes() {
            continue;
        }
        let value = run.field(assign, &v).map_err(Error::Sql)?;
        if run.is_cut(pos) {
            truncated.get_or_insert(SqlError::data_truncated());
        }
        // Bytes too long for a Raw column are refused, and nothing is written
        // (`fixtures/sql/gram_bin.json`).
        if let Value::Binary(bytes) = &value
            && assign.field_type == tflip_storage::field::FieldType::Raw
            && target
                .fields()
                .get(assign.field)
                .is_some_and(|field| bytes.len() > usize::from(field.length()))
        {
            return Err(Error::Sql(SqlError::ace(
                5071,
                "The data was too long for the destination and was truncated.",
                &format!("Field: \"{}\"", assign.name),
            )));
        }
        match target.set_field(
            FieldRef::Number(assign.field + 1),
            &set_value(assign, value),
        ) {
            Ok(()) => {}
            Err(Error::DataTruncated) => {
                truncated.get_or_insert(SqlError::truncated(&assign.name, pos));
            }
            Err(Error::NotVfpNullable) => {
                return Err(Error::Sql(SqlError::ace(
                    5205,
                    "This operation is only allowed on Nullable fields within an ADS_VFP table.",
                    &format!("Field: \"{}\"", assign.name),
                )));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(truncated)
}

/// The record an `INSERT` writes, as a trigger's `__new`: the inserted
/// columns' values in their fields, the others NULL.
fn new_record(target: &table::Table, cols: &Columns, row: &[(V, usize)]) -> Record {
    let fields = target.fields().to_vec();
    let mut values = vec![V::Null; fields.len()];
    for (assign, (v, _)) in cols.assigns.iter().zip(row) {
        if assign.takes() && assign.field < values.len() {
            values[assign.field] = v.clone();
        }
    }
    Record::new(fields, values)
}

/// The record about to be written, as a trigger's `__new`: the pending record,
/// so with the dictionary's defaults, but memo values from `set` (the
/// statement's own record), which are written with the record.
fn written_record(target: &table::Table, set: &Record) -> Record {
    match target.ri_pending_row() {
        Ok(Some((fields, mut values))) => {
            for (i, field) in fields.iter().enumerate() {
                if field.is_memo()
                    && let (Some(value), Some(slot)) = (set.values().get(i), values.get_mut(i))
                {
                    *slot = value.clone();
                }
            }
            Record::new(fields, values)
        }
        _ => Record::new(set.fields().to_vec(), set.values().to_vec()),
    }
}

/// The record an `UPDATE` writes, as a trigger's `__new`: the old values
/// `old` with the `SET` columns' new values `sets` applied.
fn updated_record(target: &table::Table, cols: &Columns, old: &[V], sets: &[(V, usize)]) -> Record {
    let fields = target.fields().to_vec();
    let mut values = old.to_vec();
    for (assign, (v, _)) in cols.assigns.iter().zip(sets) {
        if assign.takes() && assign.field < values.len() {
            values[assign.field] = v.clone();
        }
    }
    Record::new(fields, values)
}

/// `values` read for their columns (see [`DmlRun::read`]).
fn read(
    cols: &Columns,
    run: &DmlRun<'_>,
    values: Vec<(V, usize)>,
) -> Result<Vec<(V, usize)>, Error> {
    let mut out = Vec::with_capacity(values.len());
    for (assign, (v, pos)) in cols.assigns.iter().zip(values) {
        let v = match assign.takes() {
            true => run.read(assign, v, pos).map_err(Error::Sql)?,
            false => v,
        };
        out.push((v, pos));
    }
    Ok(out)
}

/// Keeps what the record `recno` just written holds for `LASTAUTOINC`,
/// `LASTROWID` and `LASTROWVERSION` (`fixtures/sql/rowfuncs.json`): its
/// AutoInc value and `ROWID` if it is an insert, its ROWVERSION if the table
/// has one.
fn remember(fire: &Fire, target: &table::Table, recno: u32, insert: bool) {
    let Ok(Some((fields, values))) = target.stored_row() else {
        return;
    };
    let number = |field_type| {
        let i = fields.iter().position(|f| f.field_type == field_type)?;
        values.get(i)?.as_i128()
    };
    let last = crate::statement::last();
    let mut last = crate::lock(&last);
    if insert {
        if let Some(n) = number(FieldType::AutoInc) {
            last.autoinc = i32::try_from(n).ok();
            if let Ok(owner) = crate::connection::get(fire.session, fire.connection) {
                *crate::lock(&owner.last_autoinc) = last.autoinc;
            }
        }
        if let Some(prefix) = target.sql_info().rowid_prefix {
            last.rowid = Some(tflip_sql::rowid::encode(&prefix, recno));
        }
    }
    if let Some(n) = number(FieldType::RowVersion) {
        last.rowversion = i64::try_from(n).ok();
    }
}

/// Writes the current record; a key a unique order refuses is the original
/// engine's SQL error naming the order, by the table for an ADT table and
/// by the index file (with its extension) for a DBF one
/// (`fixtures/sql/dbfpk`).
fn write(target: &table::Table, cols: &Columns) -> Result<(), Error> {
    match target.write_record() {
        Err(Error::UniqueViolation(name)) => {
            let tag = target.refused_by().unwrap_or_default().to_ascii_uppercase();
            let file = match (target.dbf_flavour(), name.split_once(':')) {
                (Some(_), Some((file, _))) => file,
                _ => cols.table,
            };
            Err(Error::Sql(SqlError::not_unique(file, &tag)))
        }
        result => result,
    }
}

/// Whether `error` is the refusal of a key by an order: one a unique order
/// has already (what [`write`] makes of [`Error::UniqueViolation`]), or a
/// NULL one a candidate order cannot take.
fn is_key_refused(error: &Error) -> bool {
    match error {
        Error::Sql(error) => error.native == 7057,
        Error::Constraint { code, .. } => *code == 5208,
        _ => false,
    }
}

/// How the engine sets `value` in the column of `assign`.
fn set_value(assign: &Assign, value: Value) -> SetValue {
    set_value_in(assign.field_type, value)
}

/// How the engine sets `value` in a field of type `field_type`.
pub(crate) fn set_value_in(field_type: tflip_storage::field::FieldType, value: Value) -> SetValue {
    match value {
        Value::Null => SetValue::Null,
        Value::Blank => SetValue::Empty,
        Value::Text(s) => SetValue::String(s),
        Value::Binary(b) => match field_type {
            tflip_storage::field::FieldType::Image => SetValue::Image(b),
            // A binary literal in a Raw column: `AdsSetBinary` takes none.
            tflip_storage::field::FieldType::Raw => SetValue::Raw(b),
            _ => SetValue::Binary(b),
        },
        Value::Logical(b) => SetValue::Logical(b),
        Value::Numeric(text) => SetValue::Field(text),
        Value::Integer(n) => SetValue::LongLong(n as i64),
        Value::Double(d) => SetValue::Double(d),
        Value::Money(m) => SetValue::Money(m),
        Value::Date(d) => SetValue::Julian(d),
        Value::Time(t) => SetValue::Milliseconds(t as i32),
        Value::TimeStamp(d, t) => SetValue::TimeStampRaw(d, t),
        Value::Guid(g) => SetValue::Field(Value::guid_text(&g)),
    }
}
