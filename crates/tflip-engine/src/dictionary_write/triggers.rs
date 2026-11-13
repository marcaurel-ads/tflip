//! Adding and removing triggers.

use super::*;

/// Writes a table object's aggregate trigger slots: slot 9 the OR of its
/// triggers' trigger types, slot 10 the OR of their options, which the engine
/// keeps on the table so it knows at a glance what triggers fire. `table_record`
/// is the table's 1-based record.
fn set_table_trigger_aggregates(
    add_path: &Path,
    table_record: u32,
    types: u32,
    options: u32,
) -> Result<(), Error> {
    use tflip_storage::dd;

    let mut file = open_dictionary_file(add_path)?;
    edit_object_properties(&mut file, add_path, table_record, |slots| {
        set_slot(slots, dd::table::TRIGGER_TYPES, Some(&types.to_le_bytes()));
        set_slot(
            slots,
            dd::table::TRIGGER_OPTIONS,
            Some(&options.to_le_bytes()),
        );
    })
}

/// What a trigger of `event` (insert 1, update 2, delete 3), of `trigger_type`
/// and `options`, adds to its table's aggregate trigger types and options: the
/// types take three bits for each event, the options four
/// (`fixtures/sql/spddtrig`).
fn trigger_bits(event: u32, trigger_type: u32, options: u32) -> (u32, u32) {
    let at = event.saturating_sub(1).min(2);
    (trigger_type << (3 * at), options << (4 * at))
}

/// Adds a trigger to the data dictionary `connection` is to
/// (`AdsDDCreateTrigger`): a Trigger object under the table it fires on, holding
/// the event and trigger types, the container type, the container (a SQL script
/// or an external library), the function name, the priority and the options,
/// with its comment in the dictionary memo — written as the original engine
/// writes it. The table's aggregate trigger-type and option slots are updated to
/// take in the new trigger, and the dictionary version is raised to 4 (triggers
/// need a newer engine). An empty or already-used name is refused with
/// `AE_INVALID_OBJECT_NAME` (5132); an unknown table with the same.
#[allow(clippy::too_many_arguments)]
pub fn add_trigger_api(
    session: &Session,
    connection: Handle,
    name: &str,
    table_name: &str,
    trigger_type: u32,
    event_types: u32,
    container_type: u32,
    container: &str,
    function_name: &str,
    priority: u32,
    comment: &str,
    options: u32,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind, create::NewObject};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    conn.trigger_order_sync();
    if name.is_empty() {
        return Err(invalid_object_name(name));
    }
    super::views_procedures::check_trigger_script_length(container)?;
    let dict = open_dict(&add_path)?;
    // A database trigger (no table) hangs under the Database object.
    let table = match table_name.is_empty() {
        true => None,
        false => Some(find_named(&dict, Kind::Table, "table", table_name)?),
    };
    let table_id = match table {
        Some(table) => table.id,
        None => dict.database().map_or(1, |db| db.id),
    };
    // A name is the trigger's alone among those of its table, or of the
    // database (probed, `fixtures/sql/trigdb.json`).
    if dict.child(table_id, Kind::Trigger, name).is_some() {
        return Err(invalid_object_name(name));
    }
    let (table_record, types, options_agg) = match table {
        Some(table) => {
            let (add_types, add_options) = trigger_bits(event_types, trigger_type, options);
            (
                Some(table.record()),
                table.u32(dd::table::TRIGGER_TYPES).unwrap_or(0) | add_types,
                table.u32(dd::table::TRIGGER_OPTIONS).unwrap_or(0) | add_options,
            )
        }
        None => (None, 0, 0),
    };
    drop(dict);

    let next_id = read_next_id(&add_path)?;
    let event = event_types.to_le_bytes();
    let ttype = trigger_type.to_le_bytes();
    let ctype = container_type.to_le_bytes();
    let container_z = nul_bytes(container);
    let func_z = nul_bytes(function_name);
    let prio = priority.to_le_bytes();
    let opts = options.to_le_bytes();
    let mut slots: Vec<(usize, &[u8])> = vec![
        (dd::trigger::EVENT_TYPE, &event),
        (dd::trigger::TRIGGER_TYPE, &ttype),
        (dd::trigger::CONTAINER_TYPE, &ctype),
        (dd::trigger::CONTAINER, &container_z),
    ];
    if !function_name.is_empty() {
        slots.push((dd::trigger::FUNCTION_NAME, &func_z));
    }
    slots.push((dd::trigger::PRIORITY, &prio));
    slots.push((dd::trigger::OPTIONS, &opts));
    // A long script goes on in `More Property`, as a long view's statement
    // does.
    let object = NewObject {
        id: next_id,
        parent: table_id as i32,
        kind: "Trigger",
        name,
        info1: table_id as i32, // the table the trigger fires on
        info2: 0x8000_0000,
        slots: &slots,
    };
    let mut record = dd::create::object_record_stepwise(
        &object,
        &dd::memo_path(&add_path),
        &[
            dd::trigger::EVENT_TYPE,
            dd::trigger::TRIGGER_TYPE,
            dd::trigger::CONTAINER_TYPE,
            dd::trigger::CONTAINER,
            dd::trigger::FUNCTION_NAME,
            dd::trigger::PRIORITY,
            dd::trigger::OPTIONS,
        ],
    )
    .map_err(|_| Error::CorruptTable)?;
    dd::create::set_comment_strlen(&mut record, &dd::memo_path(&add_path), comment.as_bytes())
        .map_err(|_| Error::CorruptTable)?;
    dd::create::insert_objects(&add_path, &[record], next_id + 1)
        .map_err(|_| Error::CorruptTable)?;
    if let Some(table_record) = table_record {
        set_table_trigger_aggregates(&add_path, table_record, types, options_agg)?;
    }
    raise_trigger_version(&add_path)?;
    rebuild_dictionary(session, &conn, &add_path)
}

/// `ALTER TRIGGER`: rewrites the trigger `name` of the table `table_name` (of the
/// database if that is empty) in its record, which keeps its id, with the event
/// and trigger types, the script, the priority and the options it is given,
/// and its table's aggregate trigger-type and option slots follow
/// (`fixtures/sql/trigdb.json`). One that is not there is refused with 5132.
#[allow(clippy::too_many_arguments)]
pub fn alter_trigger(
    session: &Session,
    connection: Handle,
    name: &str,
    table_name: &str,
    trigger_type: u32,
    event_types: u32,
    container_type: u32,
    container: &str,
    function_name: &str,
    priority: u32,
    options: u32,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    conn.trigger_order_sync();
    super::views_procedures::check_trigger_script_length(container)?;
    let dict = open_dict(&add_path)?;
    let table = match table_name.is_empty() {
        true => None,
        false => Some(find_named(&dict, Kind::Table, "table", table_name)?),
    };
    let parent = match table {
        Some(table) => table.id,
        None => dict.database().map_or(1, |db| db.id),
    };
    let found = find_child(&dict, parent, Kind::Trigger, "trigger", name)?;
    let (record, id) = (found.record(), found.id);
    // The aggregates of the triggers of the table, this one as it is made.
    let mut aggregates = (0u32, 0u32);
    if table.is_some() {
        for other in dict.all(Kind::Trigger) {
            if other.parent != parent {
                continue;
            }
            let (event, kind, opts) = match other.record() == record {
                true => (event_types, trigger_type, options),
                false => (
                    other.u32(dd::trigger::EVENT_TYPE).unwrap_or(1),
                    other.u32(dd::trigger::TRIGGER_TYPE).unwrap_or(0),
                    other.u32(dd::trigger::OPTIONS).unwrap_or(0),
                ),
            };
            let (types, options) = trigger_bits(event, kind, opts);
            aggregates.0 |= types;
            aggregates.1 |= options;
        }
    }
    let table_record = table.map(|t| t.record());
    drop(dict);

    let event = event_types.to_le_bytes();
    let ttype = trigger_type.to_le_bytes();
    let ctype = container_type.to_le_bytes();
    let container_z = nul_bytes(container);
    let func_z = nul_bytes(function_name);
    let prio = priority.to_le_bytes();
    let opts = options.to_le_bytes();
    {
        let mut file = open_dictionary_file(&add_path)?;
        edit_object_properties(&mut file, &add_path, record, |slots| {
            set_slot(slots, dd::trigger::EVENT_TYPE, Some(&event));
            set_slot(slots, dd::trigger::TRIGGER_TYPE, Some(&ttype));
            set_slot(slots, dd::trigger::CONTAINER_TYPE, Some(&ctype));
            set_slot(slots, dd::trigger::CONTAINER, Some(&container_z));
            set_slot(
                slots,
                dd::trigger::FUNCTION_NAME,
                (!function_name.is_empty()).then_some(&func_z[..]),
            );
            set_slot(slots, dd::trigger::PRIORITY, Some(&prio));
            set_slot(slots, dd::trigger::OPTIONS, Some(&opts));
        })?;
    }
    if let Some(table_record) = table_record {
        set_table_trigger_aggregates(&add_path, table_record, aggregates.0, aggregates.1)?;
    }
    raise_trigger_version(&add_path)?;
    rebuild_dictionary(session, &conn, &add_path)?;
    // As the original's copy takes it out and puts it in again.
    conn.trigger_order_altered(id);
    Ok(())
}

/// Removes a trigger from the data dictionary (`AdsDDRemoveTrigger`):
/// soft-deletes the Trigger object and recomputes its table's aggregate
/// trigger-type and option slots from the triggers that remain — the exact
/// inverse of what [`add_trigger_api`] writes.
///
/// The original finds a trigger by its qualified name, `table::trigger` (the
/// one `AdsDDFindFirstObject` lists), and answers 5132, "The trigger is not
/// part of the database.", for a bare name, a table or a trigger it does not
/// have. A user who is not the administrator needs `ALTER` on the table, which
/// is looked at after the names (`fixtures/sql/ddapirights2`).
pub fn remove_trigger_api(session: &Session, connection: Handle, name: &str) -> Result<(), Error> {
    use tflip_storage::dd::Kind;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let not_part = || ace_error::sql(5132, "The trigger is not part of the database.");
    let (table_name, trigger_name) = name.split_once("::").ok_or_else(not_part)?;
    let dict = open_dict(&add_path)?;
    let table = dict.named(Kind::Table, table_name).ok_or_else(not_part)?;
    let trigger = dict
        .child(table.id, Kind::Trigger, trigger_name)
        .ok_or_else(not_part)?;
    let (table_id, table_name, trigger_name) = (table.id, table.name.clone(), trigger.name.clone());
    drop(dict);
    super::rights::require(&conn, 1, &table_name, super::rights::ALTER, "")?;
    remove_trigger(session, connection, Some(table_id), &trigger_name)
}

/// [`remove_trigger_api`] for the trigger `name` of the object numbered
/// `parent` (a table, or the database for a database trigger), else the first
/// of that name.
pub fn remove_trigger(
    session: &Session,
    connection: Handle,
    parent: Option<u32>,
    name: &str,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    conn.trigger_order_sync();
    let dict = open_dict(&add_path)?;
    let trigger = match parent {
        Some(parent) => find_child(&dict, parent, Kind::Trigger, "trigger", name)?,
        None => find_named(&dict, Kind::Trigger, "trigger", name)?,
    };
    let table_id = trigger.parent;
    let record = trigger.record();
    let table_record = dict
        .all(Kind::Table)
        .into_iter()
        .find(|t| t.id == table_id)
        .map(|t| t.record());
    // The table's aggregates are the OR of every trigger on it, so removing one
    // means recomputing them from the triggers that remain, not clearing bits
    // this trigger may share with another.
    let (mut types, mut options) = (0u32, 0u32);
    for other in dict.all(Kind::Trigger) {
        if other.parent == table_id && other.record() != record {
            let (add_types, add_options) = trigger_bits(
                other.u32(dd::trigger::EVENT_TYPE).unwrap_or(1),
                other.u32(dd::trigger::TRIGGER_TYPE).unwrap_or(0),
                other.u32(dd::trigger::OPTIONS).unwrap_or(0),
            );
            types |= add_types;
            options |= add_options;
        }
    }
    drop(dict);

    if let Some(table_record) = table_record {
        set_table_trigger_aggregates(&add_path, table_record, types, options)?;
    }
    delete_records(session, &conn, &add_path, &[record])
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn adds_triggers_like_the_engine() {
        let session = Session::new();
        let fx = ops_fixtures();
        let add_two = |session: &Session, connect: Handle| {
            add_trigger_api(
                session,
                connect,
                "items_bi",
                "items",
                1,
                1,
                3,
                " INSERT INTO items VALUES (1); ",
                "",
                2,
                "bi note",
                3,
            )
            .unwrap();
            add_trigger_api(
                session,
                connect,
                "items_ai",
                "items",
                4,
                1,
                3,
                " SELECT 1; ",
                "",
                1,
                "",
                1,
            )
            .unwrap();
        };
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddtrigadd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        copy_free(&fx.join("trig_items.adt"), &dir.join("items.adt"));
        add_table_api(
            &session,
            connect,
            "items",
            "items.adt",
            TableType::Adt,
            1,
            "",
            "",
        )
        .unwrap();
        add_two(&session, connect);
        for (file, golden) in [
            ("db.add", "triggadd.add"),
            ("db.am", "triggadd.am"),
            ("db.ai", "triggadd.ai"),
        ] {
            let ours = std::fs::read(dir.join(file)).unwrap();
            let want = std::fs::read(fx.join(golden)).unwrap();
            let (ours, want) = if file == "db.add" {
                (mask_login(ours), mask_login(want))
            } else {
                (ours, want)
            };
            assert_eq!(ours, want, "{file}");
        }
        // An empty, already-used, or unknown-table trigger is refused.
        assert!(matches!(
            add_trigger_api(
                &session, connect, "items_bi", "items", 1, 1, 3, " X ", "", 1, "", 1
            ),
            Err(Error::Sql(_))
        ));
        assert!(matches!(
            add_trigger_api(
                &session, connect, "t_x", "nope", 1, 1, 3, " X ", "", 1, "", 1
            ),
            Err(Error::Sql(_))
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `AdsDDRemoveTrigger` soft-deletes a trigger and recomputes its table's
    /// aggregate slots from the triggers that remain. The engine's own version
    /// fails 5132 on a trigger it wrote, so this is a tflip round-trip.
    #[test]
    fn removes_a_trigger() {
        use tflip_storage::dd::{self, Dictionary, Kind};
        let session = Session::new();
        let fx = ops_fixtures();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddtrigrem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        copy_free(&fx.join("trig_items.adt"), &dir.join("items.adt"));
        add_table_api(
            &session,
            connect,
            "items",
            "items.adt",
            TableType::Adt,
            1,
            "",
            "",
        )
        .unwrap();
        // Two triggers: types 1 and 4, options 3 and 1 — the table aggregates
        // are their OR (5 and 3).
        add_trigger_api(
            &session, connect, "items_bi", "items", 1, 1, 3, " X ", "", 2, "", 3,
        )
        .unwrap();
        add_trigger_api(
            &session, connect, "items_ai", "items", 4, 1, 3, " Y ", "", 1, "", 1,
        )
        .unwrap();
        let aggregates = |add: &Path| {
            let dict = Dictionary::open(add).unwrap();
            let t = dict.named(Kind::Table, "items").unwrap();
            (
                t.u32(dd::table::TRIGGER_TYPES).unwrap_or(0),
                t.u32(dd::table::TRIGGER_OPTIONS).unwrap_or(0),
            )
        };
        assert_eq!(aggregates(&add), (5, 3));

        // Removing items_bi leaves items_ai; the aggregates fall to its bits.
        remove_trigger_api(&session, connect, "items::items_bi").unwrap();
        {
            let dict = Dictionary::open(&add).unwrap();
            assert!(dict.named(Kind::Trigger, "items_bi").is_none());
            assert!(dict.named(Kind::Trigger, "items_ai").is_some());
        }
        assert_eq!(aggregates(&add), (4, 1));

        // Removing the last trigger clears the aggregates.
        remove_trigger_api(&session, connect, "items::items_ai").unwrap();
        assert_eq!(aggregates(&add), (0, 0));

        // An unknown trigger is refused.
        assert!(matches!(
            remove_trigger_api(&session, connect, "nope"),
            Err(Error::Sql(_))
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
