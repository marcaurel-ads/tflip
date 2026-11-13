//! Adding and removing referential integrity rules.

use super::*;

/// Numbers the referential integrity graphs anew, as the original engine keeps
/// them: the tables that rules join, and the rules, form graphs, each known by
/// the id of one rule. It is stored in each rule's `Info1` and in the RI-graph
/// slot (slot 5) of each table of the graph. A rule joins the graph its parent
/// table is in, or starts one of its own id, and brings in the graph its child
/// table is in. A table left in no rule keeps its slot, empty
/// (`fixtures/sql/spddri`).
pub(super) fn renumber_ri_graphs(add_path: &Path) -> Result<(), Error> {
    use std::collections::HashMap;
    use tflip_storage::dd::{self, Kind};

    let dict = open_dict(add_path)?;
    let mut rules = dict.all(Kind::Relation);
    rules.sort_by_key(|rule| rule.id);
    let mut of_table: HashMap<u32, u32> = HashMap::new();
    let mut of_rule: Vec<(u32, u32, Option<i32>)> = Vec::new();
    for rule in rules {
        let (Some(parent), Some(child)) = (
            rule.u32(dd::relation::PRIMARY_TABLE),
            rule.u32(dd::relation::FOREIGN_TABLE),
        ) else {
            continue;
        };
        let graph = of_table.get(&parent).copied().unwrap_or(rule.id);
        if let Some(&other) = of_table.get(&child).filter(|&&other| other != graph) {
            for id in of_table
                .values_mut()
                .chain(of_rule.iter_mut().map(|r| &mut r.1))
            {
                if *id == other {
                    *id = graph;
                }
            }
        }
        of_table.insert(parent, graph);
        of_table.insert(child, graph);
        of_rule.push((rule.record(), graph, rule.info1));
    }
    let mut file = open_dictionary_file(add_path)?;
    for (record, graph, was) in of_rule {
        if was != Some(graph as i32) {
            file.write_part(record, INFO1, &graph.to_le_bytes())
                .map_err(|_| Error::CorruptTable)?;
        }
    }
    for table in dict.all(Kind::Table) {
        let want: Option<Vec<u8>> = match of_table.get(&table.id) {
            Some(graph) => Some(graph.to_le_bytes().to_vec()),
            None if table.slot(dd::table::RI_GRAPH).is_some() => Some(vec![0; 4]),
            None => None,
        };
        if want.as_deref() != table.slot(dd::table::RI_GRAPH) {
            edit_object_properties(&mut file, add_path, table.record(), |slots| {
                set_slot(slots, dd::table::RI_GRAPH, want.as_deref());
            })?;
        }
    }
    Ok(())
}

/// Adds a referential-integrity rule to the data dictionary `connection` is to
/// (`AdsDDCreateRefIntegrity`/`AdsDDCreateRefIntegrity62`): a Relation object
/// under the Database (its `Info1` its own id) holding the parent and child table
/// and index ids, the update and delete rules, and the no-primary and cascade
/// error messages, with both tables' RI-graph slots pointing at it — written as
/// the original engine writes it (the Relation record and the RI-graph slots are
/// verified byte for byte against a full-engine golden). The dictionary version
/// is raised to 4. A reused rule name is refused with `AE_INVALID_OBJECT_NAME`
/// (5132); the parent/child tables and the child tag with 5132 and their own
/// messages; a parent table with no designated primary key with
/// `AE_INVALID_OBJECT_ID` (5131); and a parent tag that is not that primary
/// index with 5143 — matching the original engine's checks and wording (probed
/// black-box; see `docs/ADS_BUGS.md`).
#[allow(clippy::too_many_arguments)]
pub fn add_refintegrity_api(
    session: &Session,
    connection: Handle,
    ri_name: &str,
    parent_table: &str,
    parent_tag: &str,
    child_table: &str,
    child_tag: &str,
    update_rule: u8,
    delete_rule: u8,
    no_primary_error: &str,
    cascade_error: &str,
) -> Result<(), Error> {
    add_refintegrity(
        session,
        connection,
        [ri_name, parent_table, parent_tag, child_table, child_tag],
        (update_rule, delete_rule),
        (no_primary_error, cascade_error),
        true,
    )
}

/// [`add_refintegrity_api`], which raises the dictionary's version (the system
/// procedure does not: `fixtures/sql/spddri`). The names are the rule's,
/// the parent table's and its tag, the child table's and its tag.
pub(crate) fn add_refintegrity(
    session: &Session,
    connection: Handle,
    [ri_name, parent_table, parent_tag, child_table, child_tag]: [&str; 5],
    (update_rule, delete_rule): (u8, u8),
    (no_primary_error, cascade_error): (&str, &str),
    raise_version: bool,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind, create::NewObject};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    if ri_name.is_empty() {
        return Err(invalid_object_name(ri_name));
    }
    let dict = open_dict(&add_path)?;
    if dict.named(Kind::Relation, ri_name).is_some() {
        return Err(invalid_object_name(ri_name));
    }
    // The engine checks, in order and with its own messages: the parent table
    // exists (5132), it has a designated primary key (5131), `parent_tag` names
    // that primary index (5143, an unknown tag included), the child table
    // exists (5132), and `child_tag` names an index of it (5132).
    let parent = dict.named(Kind::Table, parent_table).ok_or_else(|| {
        ace_error::sql(
            5132,
            "The primary key table name is not in the data dictionary.",
        )
    })?;
    let primary_id = parent.info1.filter(|&i| i != 0).ok_or_else(|| {
        // 5131's detail has no trailing period, as the engine writes it.
        ace_error::sql(5131, "No primary key was found for the given table")
    })? as u32;
    let parent_index = dict
        .child(parent.id, Kind::Index, parent_tag)
        .filter(|i| i.id == primary_id)
        .ok_or_else(|| {
            ace_error::sql(
                5143,
                "The primary key name supplied is not the primary index for the table.",
            )
        })?;
    let child = dict.named(Kind::Table, child_table).ok_or_else(|| {
        ace_error::sql(
            5132,
            "The foreign key table name is not in the data dictionary.",
        )
    })?;
    let child_index = dict
        .child(child.id, Kind::Index, child_tag)
        .ok_or_else(|| {
            ace_error::sql(5132, "The foreign key index is not in the data dictionary.")
        })?;
    let (primary_table, primary_index) = (parent.id, parent_index.id);
    let (foreign_table, foreign_index) = (child.id, child_index.id);
    drop(dict);

    const DATABASE_ID: i32 = 1;
    let next_id = read_next_id(&add_path)?;
    let primary_t = primary_table.to_le_bytes();
    let primary_i = primary_index.to_le_bytes();
    let foreign_t = foreign_table.to_le_bytes();
    let foreign_i = foreign_index.to_le_bytes();
    let update = [update_rule];
    let delete = [delete_rule];
    let no_pkey = nul_bytes(no_primary_error);
    let cascade = nul_bytes(cascade_error);
    let mut slots: Vec<(usize, &[u8])> = vec![
        (dd::relation::PRIMARY_TABLE, &primary_t),
        (dd::relation::PRIMARY_INDEX, &primary_i),
        (dd::relation::FOREIGN_TABLE, &foreign_t),
        (dd::relation::FOREIGN_INDEX, &foreign_i),
        (dd::relation::UPDATE_RULE, &update),
        (dd::relation::DELETE_RULE, &delete),
    ];
    if !no_primary_error.is_empty() {
        slots.push((dd::relation::NO_PKEY_ERROR, &no_pkey));
    }
    if !cascade_error.is_empty() {
        slots.push((dd::relation::CASCADE_ERROR, &cascade));
    }
    // Messages too long for the `Property` field go on in the memo file.
    let record = dd::create::object_record_continued_once(
        &NewObject {
            id: next_id,
            parent: DATABASE_ID,
            kind: "Relation",
            name: ri_name,
            info1: next_id as i32, // a relation's Info1 is its own id
            info2: 0x8000_0000,
            slots: &slots,
        },
        &dd::memo_path(&add_path),
    )
    .map_err(|_| Error::CorruptTable)?;
    dd::create::insert_objects(&add_path, &[record], next_id + 1)
        .map_err(|_| Error::CorruptTable)?;
    renumber_ri_graphs(&add_path)?;
    if raise_version {
        raise_dictionary_version(&add_path, 4, b"8.1\0")?;
    }
    rebuild_dictionary(session, &conn, &add_path)
}

/// Removes a referential-integrity rule from the data dictionary `connection` is
/// to (`AdsDDRemoveRefIntegrity`): soft-deletes its Relation object and numbers the
/// RI graphs anew. A name that is no rule fails with
/// `AE_INVALID_OBJECT_NAME` (5132).
pub fn remove_refintegrity_api(
    session: &Session,
    connection: Handle,
    ri_name: &str,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let Some(rule) = dict.named(Kind::Relation, ri_name) else {
        return Err(not_valid("The referential integrity could not be found."));
    };
    // A rule is changed by a user who may alter both its tables.
    let tables = [dd::relation::PRIMARY_TABLE, dd::relation::FOREIGN_TABLE]
        .map(|slot| rule.u32(slot).and_then(|id| dict.object(id)));
    if !tables
        .iter()
        .flatten()
        .all(|table| super::rights::may(&conn, 1, &table.name, super::rights::ALTER))
    {
        return Err(super::rights::refuse(""));
    }
    let record = rule.record();
    drop(dict);
    delete_records(session, &conn, &add_path, &[record])?;
    renumber_ri_graphs(&add_path)?;
    rebuild_dictionary(session, &conn, &add_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_a_relation_record_like_the_engine() {
        // The one full-engine dictionary that holds a relation (props): its
        // Relation object is record 18 (child_to_parent). Rebuilding it from the
        // same inputs must give the same bytes, which pins the Relation layout.
        use tflip_storage::dd::{self, create::NewObject};
        let props = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/adt/dictionary_props/props.add");
        let bytes = std::fs::read(&props).unwrap();
        let data = u32::from_le_bytes(bytes[0x20..0x24].try_into().unwrap()) as usize;
        let golden = &bytes[data + 17 * 524..data + 18 * 524];

        let (pt, pi, ft, fi) = (2u32, 13u32, 7u32, 15u32);
        let (primary_t, primary_i) = (pt.to_le_bytes(), pi.to_le_bytes());
        let (foreign_t, foreign_i) = (ft.to_le_bytes(), fi.to_le_bytes());
        let (update, delete) = ([1u8], [2u8]);
        let no_pkey = nul_bytes("no parent");
        let cascade = nul_bytes("cascade failed");
        let slots: Vec<(usize, &[u8])> = vec![
            (dd::relation::PRIMARY_TABLE, &primary_t),
            (dd::relation::PRIMARY_INDEX, &primary_i),
            (dd::relation::FOREIGN_TABLE, &foreign_t),
            (dd::relation::FOREIGN_INDEX, &foreign_i),
            (dd::relation::UPDATE_RULE, &update),
            (dd::relation::DELETE_RULE, &delete),
            (dd::relation::NO_PKEY_ERROR, &no_pkey),
            (dd::relation::CASCADE_ERROR, &cascade),
        ];
        let ours = dd::create::object_record(&NewObject {
            id: 18,
            parent: 1,
            kind: "Relation",
            name: "child_to_parent",
            info1: 18,
            info2: 0x8000_0000,
            slots: &slots,
        });
        assert_eq!(ours, golden, "Relation record 18");
    }

    #[test]
    fn creates_and_removes_referential_integrity() {
        use tflip_storage::dd::{self, Dictionary, Kind};
        let session = Session::new();
        let props =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt/dictionary_props");
        let dir =
            tflip_storage::testing::temp_root().join(format!("tflip-ddri-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        for t in ["parent", "child"] {
            super::test_support::copy_free(
                &props.join(format!("{t}.adt")),
                &dir.join(format!("{t}.adt")),
            );
            std::fs::copy(props.join(format!("{t}.adi")), dir.join(format!("{t}.adi"))).unwrap();
            // The child table has a memo file; copy it when present.
            let _ = std::fs::copy(props.join(format!("{t}.adm")), dir.join(format!("{t}.adm")));
            add_table_api(
                &session,
                connect,
                t,
                &format!("{t}.adt"),
                TableType::Adt,
                1,
                &format!("{t}.adi"),
                "",
            )
            .unwrap();
        }
        // The engine requires the parent to have a designated primary key before
        // a referential-integrity rule can reference it; designate PARENT_ID.
        // ADS_DD_TABLE_PRIMARY_KEY (202) stores that index's id in the table's Info1.
        set_table_property_api(&session, connect, "parent", 202, b"PARENT_ID\0").unwrap();
        {
            let dict = Dictionary::open(&add).unwrap();
            let parent = dict.named(Kind::Table, "parent").unwrap();
            let index = dict.child(parent.id, Kind::Index, "PARENT_ID").unwrap();
            assert_eq!(
                parent.info1,
                Some(index.id as i32),
                "primary key index id in Info1"
            );
        }
        add_refintegrity_api(
            &session,
            connect,
            "child_to_parent",
            "parent",
            "PARENT_ID",
            "child",
            "CHILD_PARENT",
            1,
            2,
            "no parent",
            "cascade failed",
        )
        .unwrap();

        // The relation reads back with the right references, rules and errors, and
        // both tables' RI-graph slots point at it.
        {
            let dict = Dictionary::open(&add).unwrap();
            let rule = dict.named(Kind::Relation, "child_to_parent").unwrap();
            assert_eq!(rule.parent, 1);
            assert_eq!(rule.info1, Some(rule.id as i32));
            let parent = dict.named(Kind::Table, "parent").unwrap();
            let child = dict.named(Kind::Table, "child").unwrap();
            assert_eq!(rule.u32(dd::relation::PRIMARY_TABLE), Some(parent.id));
            assert_eq!(rule.u32(dd::relation::FOREIGN_TABLE), Some(child.id));
            assert_eq!(
                rule.u32(dd::relation::PRIMARY_INDEX),
                Some(dict.child(parent.id, Kind::Index, "PARENT_ID").unwrap().id)
            );
            assert_eq!(rule.u8(dd::relation::UPDATE_RULE), Some(1));
            assert_eq!(rule.u8(dd::relation::DELETE_RULE), Some(2));
            assert_eq!(
                rule.text(dd::relation::NO_PKEY_ERROR).as_deref(),
                Some("no parent")
            );
            assert_eq!(
                rule.text(dd::relation::CASCADE_ERROR).as_deref(),
                Some("cascade failed")
            );
            let rid = rule.id.to_le_bytes().to_vec();
            assert_eq!(parent.slot(dd::table::RI_GRAPH), Some(rid.as_slice()));
            assert_eq!(child.slot(dd::table::RI_GRAPH), Some(rid.as_slice()));
        }
        // A reused name, or an unknown table or tag, is refused.
        assert!(matches!(
            add_refintegrity_api(
                &session,
                connect,
                "child_to_parent",
                "parent",
                "PARENT_ID",
                "child",
                "CHILD_PARENT",
                1,
                2,
                "",
                ""
            ),
            Err(Error::Sql(_))
        ));
        assert!(matches!(
            add_refintegrity_api(
                &session,
                connect,
                "r2",
                "parent",
                "NOPE",
                "child",
                "CHILD_PARENT",
                1,
                2,
                "",
                ""
            ),
            Err(Error::Sql(_))
        ));

        // Remove takes the relation out and zeroes the tables' RI-graph slots.
        remove_refintegrity_api(&session, connect, "child_to_parent").unwrap();
        {
            let dict = Dictionary::open(&add).unwrap();
            assert!(dict.named(Kind::Relation, "child_to_parent").is_none());
            let parent = dict.named(Kind::Table, "parent").unwrap();
            assert_eq!(parent.slot(dd::table::RI_GRAPH), Some([0u8; 4].as_slice()));
        }
        assert!(matches!(
            remove_refintegrity_api(&session, connect, "child_to_parent"),
            Err(Error::Sql(_))
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checks_ref_integrity_preconditions_like_the_engine() {
        // The engine refuses AdsDDCreateRefIntegrity with specific codes and
        // messages before it writes: a missing parent/child table or child tag
        // with 5132, a parent with no primary key with 5131, and a parent tag
        // that is not the primary index with 5143 (probed black-box; see
        // docs/ADS_BUGS.md). The dictionary_ri fixture's parent `p` has primary
        // key `p_pk`; its children (cr, cc, ...) have a foreign-key index only.
        let fixtures =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt/dictionary_ri");
        let session = Session::new();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddripre-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for entry in std::fs::read_dir(&fixtures).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
        }
        let add = dir.join("ri.add");
        let connect =
            connection::connect(&session, &format!("Data Source={};", add.display())).unwrap();

        let err = |name, pt, ptag, ct, ctag| match add_refintegrity_api(
            &session, connect, name, pt, ptag, ct, ctag, 2, 2, "", "",
        ) {
            Err(Error::Sql(e)) => (e.native, e.message),
            other => panic!("expected Err(Sql), got {other:?}"),
        };

        let (code, msg) = err("n1", "nopar", "p_pk", "cr", "cr_fk");
        assert_eq!(code, 5132);
        assert!(
            msg.contains("The primary key table name is not in the data dictionary."),
            "{msg}"
        );

        let (code, msg) = err("n2", "cr", "cr_fk", "cc", "cc_fk");
        assert_eq!(code, 5131);
        assert!(
            msg.contains("No primary key was found for the given table"),
            "{msg}"
        );

        let (code, msg) = err("n3", "p", "nope", "cr", "cr_fk");
        assert_eq!(code, 5143);
        assert!(msg.contains("not the primary index for the table"), "{msg}");

        let (code, msg) = err("n4", "p", "p_pk", "nochi", "cr_fk");
        assert_eq!(code, 5132);
        assert!(
            msg.contains("The foreign key table name is not in the data dictionary."),
            "{msg}"
        );

        let (code, msg) = err("n5", "p", "p_pk", "cr", "nofk");
        assert_eq!(code, 5132);
        assert!(
            msg.contains("The foreign key index is not in the data dictionary."),
            "{msg}"
        );

        // Positive control: valid names and the real primary key pass the
        // preconditions and the rule is written.
        add_refintegrity_api(
            &session, connect, "n_ok", "p", "p_pk", "cr", "cr_fk", 2, 2, "", "",
        )
        .unwrap();

        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
