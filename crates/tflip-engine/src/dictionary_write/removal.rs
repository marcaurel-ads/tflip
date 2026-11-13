//! Removing views, tables, index files and indexes.

use super::*;

/// Removes a view from the data dictionary `connection` is to
/// (`AdsDDRemoveView`): soft-deletes its View object and the permissions on
/// it, and rebuilds the index.
pub fn remove_view_api(session: &Session, connection: Handle, name: &str) -> Result<(), Error> {
    use tflip_storage::dd::Kind;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let Some(view) = dict.named(Kind::View, name) else {
        return Err(not_valid("The view is not part of the database."));
    };
    super::rights::require(
        &conn,
        6,
        name,
        super::rights::DROP,
        "Permission to drop the view is not granted.",
    )?;
    let mut slots = vec![view.record()];
    slots.extend(permission_records(&dict, view.id));
    delete_records(session, &conn, &add_path, &slots)
}

/// Removes a table from the data dictionary `connection` is to
/// (`AdsDDRemoveTable`): soft-deletes the Table object and all that belongs to
/// it — its Fields (by name), its Index objects and their Keys, in the
/// engine's order — and rebuilds the index. With `delete_files`, the table's
/// own files go too (its `.adt`, memo and the index files named on it).
pub fn remove_table_api(
    session: &Session,
    connection: Handle,
    name: &str,
    delete_files: bool,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let folder = dictionary_folder(&add_path).to_path_buf();
    let dict = open_dict(&add_path)?;
    let table = find_named_api(&dict, Kind::Table, "table", name)?;
    super::rights::require(
        &conn,
        1,
        name,
        super::rights::DROP,
        "Table cannot be removed from the database.",
    )?;
    // A rights slot for DB:Public that does not check keeps the table in the
    // dictionary (`fixtures/sql/ddpublicbad`).
    if let Some(key) = super::connection_key(&conn, &dict)
        && dict.public_rights(table, Some(&key)) == dd::PublicRights::Invalid
    {
        return Err(ace_error::sql(
            5127,
            "Inconsistency detected while reading stored property.",
        ));
    }

    let slots = table_slots(&dict, table);
    let mut index_files = dict.children(table.id, Kind::IndexFile);
    index_files.sort_by_key(|o| o.name.to_ascii_lowercase());

    // The files to remove, read before the records go.
    let files: Vec<PathBuf> = if delete_files {
        let mut files = Vec::new();
        if let Some(path) = table.text(dd::table::RELATIVE_PATH) {
            files.push(folder.join(strip_relative(&path)));
        }
        for index in &index_files {
            if let Some(path) = index.text(dd::index_file::RELATIVE_PATH) {
                files.push(folder.join(strip_relative(&path)));
            }
        }
        files
    } else {
        Vec::new()
    };

    delete_records(session, &conn, &add_path, &slots)?;
    for file in files {
        // Connections that keep the table open for reuse let go of it.
        crate::cache::evict_path(&file);
        let _ = std::fs::remove_file(&file);
        // A table's memo file goes with it: an ADT table's, and a DBF
        // table's (a Clipper table's `.dbt`, a FoxPro or VFP table's `.fpt`;
        // `fixtures/sql/ddbf`).
        let is = |extension: &str| {
            file.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(extension))
        };
        let memo = if is("adt") {
            Some("adm")
        } else if is("dbf") {
            match crate::dictionary::table_type(table) {
                TableType::Ntx => Some("dbt"),
                _ => Some("fpt"),
            }
        } else {
            None
        };
        if let Some(memo) = memo {
            let _ = std::fs::remove_file(file.with_extension(memo));
        }
    }
    Ok(())
}

/// The records of the table `table` and all that belongs to it, in the order
/// the engine deletes them.
pub(super) fn table_slots(
    dict: &tflip_storage::dd::Dictionary,
    table: &tflip_storage::dd::Object,
) -> Vec<u32> {
    use tflip_storage::dd::Kind;
    // The engine deletes the table, then the permissions on it, then its
    // fields by their names (which is the column order but for a table whose
    // columns are not in alphabetical order, `fixtures/sql/ddbf`), then its
    // index files, then their keys (`fixtures/sql/ddviews`).
    let mut slots = vec![table.record()];
    slots.extend(permission_records(dict, table.id));
    // Its triggers go with it, after the permissions (`fixtures/sql/ddrights`).
    slots.extend(
        dict.children(table.id, Kind::Trigger)
            .iter()
            .map(|o| o.record()),
    );
    let mut fields = dict.children(table.id, Kind::Field);
    fields.sort_by_key(|o| o.name.to_ascii_lowercase());
    slots.extend(fields.iter().map(|o| o.record()));
    // Each index file goes with its orders, by name
    // (`fixtures/sql/ddoptsdict`).
    let mut index_files = dict.children(table.id, Kind::IndexFile);
    index_files.sort_by_key(|o| o.name.to_ascii_lowercase());
    let mut keys = dict.children(table.id, Kind::Index);
    keys.sort_by_key(|o| o.name.to_ascii_lowercase());
    for file in &index_files {
        slots.push(file.record());
        slots.extend(
            keys.iter()
                .filter(|k| k.info1 == Some(file.id as i32))
                .map(|k| k.record()),
        );
    }
    // Orders of no file of the table's, if there are any.
    slots.extend(
        keys.iter()
            .filter(|k| !index_files.iter().any(|f| k.info1 == Some(f.id as i32)))
            .map(|k| k.record()),
    );
    slots
}

/// The index file the dictionary `connection` is to keeps the order `tag` of
/// `table` in.
pub(crate) fn index_file_of(
    session: &Session,
    connection: Handle,
    table: &str,
    tag: &str,
) -> Option<PathBuf> {
    use tflip_storage::dd::{self, Dictionary, Kind};
    let (_, add_path) = dictionary_connection(session, connection).ok()?;
    let folder = add_path.parent()?.to_path_buf();
    let dict = Dictionary::open(&add_path).ok()?;
    let table = dict.named(Kind::Table, table)?;
    let key = dict.child(table.id, Kind::Index, tag)?;
    let file = dict.object(key.info1? as u32)?;
    let relative = file.text(dd::index_file::RELATIVE_PATH)?;
    Some(folder.join(strip_relative(&relative)))
}

/// `DROP TABLE … FROM DATABASE NO_DELETE`: removes the table `name` from the
/// dictionary `connection` is to and leaves its files, an ADT table's free
/// again.
pub(crate) fn remove_table_keeping_files(
    session: &Session,
    connection: Handle,
    name: &str,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (_, add_path) = dictionary_connection(session, connection)?;
    let folder = dictionary_folder(&add_path).to_path_buf();
    let dict = open_dict(&add_path)?;
    let table = find_named(&dict, Kind::Table, "table", name)?;
    let file = (crate::dictionary::table_type(table) == TableType::Adt)
        .then(|| table.text(dd::table::RELATIVE_PATH))
        .flatten()
        .map(|path| folder.join(strip_relative(&path)));
    drop(dict);
    remove_table_api(session, connection, name, false)?;
    if let Some(file) = file {
        crate::cache::evict_path(&file);
        dd::create::free_table(&file).map_err(|_| Error::CorruptTable)?;
    }
    Ok(())
}

/// `ALTER TABLE … DROP PRIMARY KEY` of the table `name` of the dictionary:
/// the table no longer names a primary key (`ADS_DD_TABLE_PRIMARY_KEY` set
/// empty); its index stays.
pub(crate) fn drop_primary_key(
    session: &Session,
    connection: Handle,
    name: &str,
) -> Result<(), Error> {
    set_table_property_api(session, connection, name, 202, b"")
}

/// `ALTER TABLE … ALTER COLUMN … DROP` of a rule of the field `field` of the
/// table `table` of the dictionary: the field's slot for it is unset.
pub(crate) fn drop_field_rule(
    session: &Session,
    connection: Handle,
    table: &str,
    field: &str,
    rule: tflip_sql::ast::ColumnRule,
) -> Result<(), Error> {
    use tflip_sql::ast::ColumnRule;
    use tflip_storage::dd::{self, Kind};

    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let object = find_named(&dict, Kind::Table, "table", table)?;
    let field = dict
        .child(object.id, Kind::Field, field)
        .ok_or_else(|| ace_error::sql(5132, "The specified field is not part of the table."))?;
    let slot = field.record();
    let index = match rule {
        ColumnRule::NotNull => {
            return set_object_presence(
                session,
                &conn,
                &add_path,
                slot,
                dd::field::NOT_NULL,
                false,
            );
        }
        ColumnRule::Minimum => dd::field::MIN_VALUE,
        ColumnRule::Maximum => dd::field::MAX_VALUE,
        ColumnRule::ErrorMessage => dd::field::VALIDATION_MSG,
        ColumnRule::Default => dd::field::DEFAULT_VALUE,
    };
    write_object_slots(session, &conn, &add_path, slot, &[(index, None)])
}

/// Registers the order `tag` `CREATE INDEX` made in a table of the
/// dictionary, in the index file `file` (the table's structural one if
/// `None`): its Key object, and the file's Index object if the file is new
/// to the table, else the count of its orders.
pub(crate) fn register_created_index(
    session: &Session,
    connection: Handle,
    table_name: &str,
    file: Option<&str>,
    tag: &str,
    noise: Option<&[u8]>,
) -> Result<(), Error> {
    use tflip_storage::dbf::Flavour;
    use tflip_storage::dd::{self, Kind, create::NewObject};
    use tflip_storage::order::IndexFile;
    use tflip_storage::table::Format;
    const ADS_COMPOUND: u32 = 0x02;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let folder = dictionary_folder(&add_path).to_path_buf();
    let dict = open_dict(&add_path)?;
    let table = find_named(&dict, Kind::Table, "table", table_name)?;
    let table_id = table.id;
    let table_file = table
        .text(dd::table::RELATIVE_PATH)
        .map(|p| folder.join(strip_relative(&p)))
        .ok_or(Error::CorruptTable)?;
    // The type of the index file (the Index object's `Info1`), and the
    // extension of the one the table's orders go to.
    let (format, extension, index_type, compound) = match crate::dictionary::table_type(table) {
        TableType::Ntx => (Format::Dbf(Flavour::Clipper), "ntx", 1, 0),
        TableType::Cdx => (Format::Dbf(Flavour::FoxPro), "cdx", 3, ADS_COMPOUND),
        TableType::Vfp => (Format::Dbf(Flavour::Visual), "cdx", 3, ADS_COMPOUND),
        _ => (Format::Adt, "adi", 6, ADS_COMPOUND),
    };
    // The tables of a dictionary open with the password they share.
    let source = super::rc4_tables::transient_in_dictionary(session, &conn, &table_file, format);
    let source = Arc::new(source.map_err(|_| Error::FileNotFound)?);
    let index_path = crate::ddl::order_file_ext(session, connection, &source, file, extension)?;
    let file_name = index_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut index = match source.encryption_cipher() {
        Some(cipher) => IndexFile::open_encrypted(&index_path, &cipher),
        None => IndexFile::open(&index_path),
    }
    .map_err(|_| Error::CorruptIndex)?;
    // An NTX file's one order is named after the file.
    let stem = index_path.file_stem().unwrap_or_default().to_string_lossy();
    let mut tags = index.tags(if extension == "ntx" { &stem } else { "" });
    // The dictionary names an NTX order in upper case, as a CDX tag is.
    for tag in &mut tags {
        tag.name.make_ascii_uppercase();
    }
    let ours = tags
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(tag))
        .ok_or(Error::CorruptIndex)?;
    let existing = dict
        .child(table_id, Kind::IndexFile, &file_name)
        .map(|o| (o.id, o.record()));
    let mut next_id = read_next_id(&add_path)?;
    let mut records = Vec::new();
    let index_file_id = match existing {
        Some((id, _)) => id,
        None => {
            let id = next_id;
            next_id += 1;
            let relative = format!(".\\{file_name}\0");
            let tag_count = (tags.len() as u16).to_le_bytes();
            let page_size = index.page_size().to_le_bytes();
            let slots: &[(usize, &[u8])] = &[
                (dd::index_file::RELATIVE_PATH, relative.as_bytes()),
                (1, &tag_count),
                (dd::index_file::PAGE_SIZE, &page_size),
            ];
            records.push(dd::create::object_record(&NewObject {
                id,
                parent: table_id as i32,
                kind: "Index",
                name: &file_name,
                info1: index_type,
                info2: 0x8000_0000,
                slots,
            }));
            id
        }
    };
    let fts = index.fts_block(ours).map_err(|_| Error::CorruptIndex)?;
    records.push(key_record(
        &source,
        ours,
        compound,
        table_id,
        next_id,
        index_file_id,
        &dd::memo_path(&add_path),
        fts.as_ref(),
        noise,
    )?);
    next_id += 1;
    drop(source);
    drop(dict);
    // The file's count of orders stays as it was when its object was made.
    dd::create::insert_objects(&add_path, &records, next_id).map_err(|_| Error::CorruptTable)?;
    rebuild_dictionary(session, &conn, &add_path)
}

/// The slots of a field object the constraints `rules` give: each one set,
/// the others as they were.
fn set_rules(slots: &mut Vec<Option<Vec<u8>>>, rules: &tflip_sql::ast::FieldRules) {
    use tflip_storage::dd;
    let text = |value: &Option<String>| value.as_ref().map(|v| format!("{v}\0").into_bytes());
    let given = [
        (dd::field::DEFAULT_VALUE, text(&rules.default)),
        (dd::field::NOT_NULL, rules.not_null.then(Vec::new)),
        (dd::field::MIN_VALUE, text(&rules.minimum)),
        (dd::field::MAX_VALUE, text(&rules.maximum)),
        (dd::field::VALIDATION_MSG, text(&rules.message)),
    ];
    for (index, value) in given {
        if let Some(value) = value {
            if slots.len() <= index {
                slots.resize(index + 1, None);
            }
            slots[index] = Some(value);
        }
    }
}

/// The dictionary's part of the restructure `outcome` of the ADT table
/// `table_name`, whose file is `table_path` and which `changes` asked for
/// (see [`crate::restructure::restructure_dictionary`]).
///
/// Fields that were only named again are renamed where they are, with the
/// constraints they were given. Otherwise the table's Field objects are
/// deleted (in the order of their records) and made anew, in the order a
/// table is added with, each a copy of the old one under the next object id
/// with its new number, name and definition, and the constraints it was
/// given set over its own (`fixtures/sql/ddalter`); one added has only
/// those. A field comment, which the original engine writes again, is
/// counted without its NUL. The Table object has its field count, and the
/// Key objects the names of the fields and the key lengths of the orders the
/// restructure rebuilt.
pub(crate) fn restructured_table(
    session: &Session,
    connection: Handle,
    table_name: &str,
    table_path: &Path,
    table_type: TableType,
    changes: &crate::restructure::Changes,
    outcome: &crate::restructure::Outcome,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind, create::NewObject};
    use tflip_storage::order::IndexFile;
    use tflip_storage::table::Format;

    let (conn, add_path) = dictionary_connection(session, connection)?;
    let folder = dictionary_folder(&add_path).to_path_buf();
    let memo_path = dd::memo_path(&add_path);
    let dict = open_dict(&add_path)?;
    let table = find_named(&dict, Kind::Table, "table", table_name)?;
    let (table_id, table_slot) = (table.id, table.record());
    let old_count = table.u16(dd::table::FIELD_COUNT);
    let mut old_fields = dict.children(table_id, Kind::Field);
    old_fields.sort_by_key(|o| o.info1.unwrap_or(0));
    let format = match table_type {
        TableType::Ntx => Format::Dbf(tflip_storage::dbf::Flavour::Clipper),
        TableType::Cdx => Format::Dbf(tflip_storage::dbf::Flavour::FoxPro),
        TableType::Vfp => Format::Dbf(tflip_storage::dbf::Flavour::Visual),
        _ => Format::Adt,
    };
    let source = super::rc4_tables::transient_in_dictionary(session, &conn, table_path, format)
        .map_err(|_| Error::CorruptTable)?;
    let fields = source.fields();
    // The constraints a field got: from the change of it, or its addition.
    let rules_of = |name: &str| {
        let changed = changes
            .change
            .iter()
            .find(|c| c.spec.name.eq_ignore_ascii_case(name))
            .map(|c| &c.rules);
        changed.or_else(|| {
            changes
                .add
                .iter()
                .find(|a| a.spec.name.eq_ignore_ascii_case(name))
                .map(|a| &a.rules)
        })
    };
    let definition = |field: &tflip_storage::field::Field| {
        [
            field.field_type.code().to_le_bytes(),
            super::tables::recorded_length(field).to_le_bytes(),
            field.decimals().to_le_bytes(),
        ]
        .concat()
    };

    if outcome.renamed {
        let objects: Vec<(u32, String)> = old_fields
            .iter()
            .map(|o| (o.record(), o.name.clone()))
            .collect();
        let rules: Vec<_> = fields.iter().map(|f| rules_of(&f.name).cloned()).collect();
        drop(dict);
        for ((field, (slot, name)), rules) in fields.iter().zip(&objects).zip(&rules) {
            if *name != field.name {
                write_object_name(&add_path, *slot, &field.name)?;
            }
            if let Some(rules) = rules {
                let mut set = vec![None; dd::field::VALIDATION_MSG + 1];
                set_rules(&mut set, rules);
                let slots: Vec<(usize, Option<&[u8]>)> = set
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| v.as_deref().map(|v| (i, Some(v))))
                    .collect();
                if !slots.is_empty() {
                    write_object_slots(session, &conn, &add_path, *slot, &slots)?;
                }
            }
        }
    } else {
        let mut records: Vec<Option<Vec<u8>>> = vec![None; fields.len()];
        {
            let mut file = open_dictionary_file(&add_path)?;
            let kept = |position: usize| outcome.sources.get(position).copied().flatten().is_some();
            // The old fields, in the order of their records (which a table
            // made by `CREATE TABLE` has last field first, and one that was
            // restructured first field first), either give up their
            // continuations in the memo file (the kept ones) or free their
            // memo blocks (the dropped ones), then the kept fields make
            // theirs anew last to first, and the new fields first to last
            // (`fixtures/sql/spddlongalter`, `spddlongalter2`).
            let mut released = vec![None; fields.len()];
            let mut by_record: Vec<_> = old_fields.iter().enumerate().collect();
            by_record.sort_by_key(|(_, old)| old.record());
            for (index, old) in by_record {
                match outcome.sources.iter().position(|s| *s == Some(index + 1)) {
                    None => dd::create::free_object_memos(&add_path, old.record())
                        .map_err(|_| Error::CorruptTable)?,
                    Some(position) => {
                        let mut record =
                            file.record(old.record()).map_err(|_| Error::CorruptTable)?;
                        let slots = dd::create::release_properties(&mut record, &memo_path)
                            .map_err(|_| Error::CorruptTable)?;
                        released[position] = Some((record, slots));
                    }
                }
            }
            let order = (0..fields.len())
                .rev()
                .filter(|&p| kept(p))
                .chain((0..fields.len()).filter(|&p| !kept(p)));
            for position in order {
                let field = &fields[position];
                let rules = rules_of(&field.name);
                let record = match outcome.sources.get(position).copied().flatten() {
                    Some(_) => {
                        let (mut record, released) =
                            released[position].take().ok_or(Error::CorruptTable)?;
                        let mut name = [b' '; 200];
                        let bytes = field.name.as_bytes();
                        name[..bytes.len().min(200)]
                            .copy_from_slice(&bytes[..bytes.len().min(200)]);
                        record[dd::create::NAME..dd::create::NAME + 200].copy_from_slice(&name);
                        let info1 = dd::create::INFO1;
                        record[info1..info1 + 4]
                            .copy_from_slice(&(position as i32 + 1).to_le_bytes());
                        dd::create::renew_comment(&mut record, &memo_path)
                            .map_err(|_| Error::CorruptTable)?;
                        let kept = definition(field);
                        // A DBF field's flags follow its type (a VFP
                        // Integer is binary).
                        let options = (table_type != TableType::Adt)
                            .then(|| super::dbf_field_options(field).to_le_bytes().to_vec());
                        dd::create::store_properties(&mut record, &memo_path, released, |slots| {
                            if slots.len() <= dd::field::DEFINITION {
                                slots.resize(dd::field::DEFINITION + 1, None);
                            }
                            slots[dd::field::DEFINITION] = Some(kept);
                            if let Some(options) = options {
                                if slots.len() <= dd::field::OPTIONS {
                                    slots.resize(dd::field::OPTIONS + 1, None);
                                }
                                slots[dd::field::OPTIONS] = Some(options);
                            }
                        })
                        .map_err(|_| Error::CorruptTable)?;
                        // The rules are set after the definition: a message
                        // that changes the blocks the continuation takes
                        // moves it once more (`fixtures/sql/spddlongalter`).
                        // One at a time, the maximum before the minimum
                        // (`fixtures/sql/spddlongalter3`).
                        if let Some(rules) = rules {
                            let mut given = vec![None; dd::field::VALIDATION_MSG + 1];
                            set_rules(&mut given, rules);
                            for index in [
                                dd::field::DEFAULT_VALUE,
                                dd::field::NOT_NULL,
                                dd::field::MAX_VALUE,
                                dd::field::MIN_VALUE,
                                dd::field::VALIDATION_MSG,
                            ] {
                                let Some(value) = given[index].take() else {
                                    continue;
                                };
                                dd::create::edit_properties(&mut record, &memo_path, |slots| {
                                    if slots.len() <= index {
                                        slots.resize(index + 1, None);
                                    }
                                    slots[index] = Some(value);
                                })
                                .map_err(|_| Error::CorruptTable)?;
                            }
                        }
                        record
                    }
                    None => {
                        let mut set = Vec::new();
                        if let Some(rules) = rules {
                            set_rules(&mut set, rules);
                        }
                        let kept = definition(field);
                        let options = super::dbf_field_options(field).to_le_bytes();
                        let mut slots: Vec<(usize, &[u8])> = set
                            .iter()
                            .enumerate()
                            .filter_map(|(i, v)| v.as_deref().map(|v| (i, v)))
                            .collect();
                        slots.push((dd::field::DEFINITION, &kept));
                        slots.push((dd::field::OPTIONS, &options));
                        // Set one at a time, as when a table is created
                        // (`fixtures/sql/spddlongalter`).
                        dd::create::object_record_stepwise(
                            &NewObject {
                                id: 0,
                                parent: table_id as i32,
                                kind: "Field",
                                name: &field.name,
                                info1: position as i32 + 1,
                                info2: 0x8000_0000,
                                slots: &slots,
                            },
                            &memo_path,
                            &[
                                dd::field::DEFINITION,
                                dd::field::OPTIONS,
                                dd::field::DEFAULT_VALUE,
                                dd::field::NOT_NULL,
                                dd::field::MAX_VALUE,
                                dd::field::MIN_VALUE,
                                dd::field::VALIDATION_MSG,
                            ],
                        )
                        .map_err(|_| Error::CorruptTable)?
                    }
                };
                records[position] = Some(record);
            }
        }
        // The old objects go in the order of their records.
        let mut slots: Vec<u32> = old_fields.iter().map(|o| o.record()).collect();
        slots.sort_unstable();
        drop(dict);
        delete_records_keeping_memos(session, &conn, &add_path, &slots)?;
        let mut next_id = read_next_id(&add_path)?;
        let mut made = Vec::with_capacity(records.len());
        for mut record in records.into_iter().rev().flatten() {
            record[5..9].copy_from_slice(&next_id.to_le_bytes());
            next_id += 1;
            record[0] = dd::create::object_record(&NewObject {
                id: 0,
                parent: 0,
                kind: "",
                name: "",
                info1: 0,
                info2: 0,
                slots: &[],
            })[0];
            record[1..5].fill(0);
            made.push(record);
        }
        dd::create::insert_objects(&add_path, &made, next_id).map_err(|_| Error::CorruptTable)?;
    }

    let count = (fields.len() as u16).to_le_bytes();
    if old_count != Some(fields.len() as u16) {
        write_object_slots(
            session,
            &conn,
            &add_path,
            table_slot,
            &[(dd::table::FIELD_COUNT, Some(&count))],
        )?;
    }
    // The orders follow the fields: their expressions the new names, their
    // keys the new lengths.
    // (A Clipper or FoxPro table declares its names in upper case.)
    let upper = matches!(table_type, TableType::Ntx | TableType::Cdx);
    let renamed_to: Vec<String> = changes
        .change
        .iter()
        .map(|c| match upper {
            true => c.spec.name.to_ascii_uppercase(),
            false => c.spec.name.clone(),
        })
        .collect();
    let renames: Vec<(&str, &str)> = changes
        .change
        .iter()
        .zip(&renamed_to)
        .filter(|(c, _)| !c.from.trim().eq_ignore_ascii_case(&c.spec.name))
        .map(|(c, to)| (c.from.trim(), to.as_str()))
        .collect();
    let dict = open_dict(&add_path)?;
    type Edits = Vec<(usize, Vec<u8>)>;
    let mut edits: Vec<(u32, Edits)> = Vec::new();
    for key in dict.children(table_id, Kind::Index) {
        let mut set: Vec<(usize, Vec<u8>)> = Vec::new();
        let mut expression = key.text(dd::index::EXPRESSION).unwrap_or_default();
        for slot in [dd::index::EXPRESSION, dd::index::CONDITION] {
            if let Some(text) = key.text(slot) {
                let renamed = crate::restructure::rename_words(&text, &renames);
                if renamed != text {
                    set.push((slot, format!("{renamed}\0").into_bytes()));
                }
                if slot == dd::index::EXPRESSION {
                    expression = renamed;
                }
            }
        }
        // An NTX file's one order is named after the file.
        let tags = key
            .info1
            .and_then(|id| dict.object(id as u32))
            .and_then(|file| file.text(dd::index_file::RELATIVE_PATH))
            .and_then(|relative| {
                let path = folder.join(relative.strip_prefix(".\\").unwrap_or(&relative));
                let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                let stem = if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("ntx"))
                {
                    stem.into_owned()
                } else {
                    String::new()
                };
                IndexFile::open(&path).ok().map(|file| file.tags(&stem))
            });
        let tag = tags
            .iter()
            .flatten()
            .find(|tag| tag.name.eq_ignore_ascii_case(&key.name));
        if let Some(tag) = tag {
            let length = tag.key_length.to_le_bytes();
            if key.slot(dd::index::KEY_LENGTH) != Some(&length[..]) {
                set.push((dd::index::KEY_LENGTH, length.to_vec()));
            }
            // The key's type follows the fields' new types.
            let compiled = source
                .with_field_names(&expression)
                .and_then(|names| source.compile(&names));
            if let Ok(compiled) = compiled {
                let key_type = super::tables::ads_key_type(compiled.ty()).to_le_bytes();
                if key.slot(dd::index::KEY_TYPE) != Some(&key_type[..]) {
                    set.push((dd::index::KEY_TYPE, key_type.to_vec()));
                }
            }
        }
        if !set.is_empty() {
            edits.push((key.record(), set));
        }
    }
    drop(dict);
    for (slot, set) in &edits {
        let slots: Vec<(usize, Option<&[u8]>)> =
            set.iter().map(|(i, v)| (*i, Some(v.as_slice()))).collect();
        write_object_slots(session, &conn, &add_path, *slot, &slots)?;
    }
    rebuild_dictionary(session, &conn, &add_path)
}

/// What a restructure of the table `table_name` that failed to build its
/// orders leaves in the dictionary: the objects are as they were, but the
/// original engine has used up two object ids per field and one per order
/// (`fixtures/sql/ddalterdbf`), and puts the objects back in the slots they
/// had in another order: the index files stay in theirs, and the others
/// take the orders' Key objects last to first, then the fields
/// (`fixtures/sql/ddalterfail`, `spddalterfail2`).
pub(crate) fn failed_restructure(
    session: &Session,
    connection: Handle,
    table_name: &str,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let table = find_named(&dict, Kind::Table, "table", table_name)?;
    // Last record to first, which is the Key objects last to first and the
    // fields first to last of a table that was made, and the reverse of
    // each after a failure before (`fixtures/write/restructure_fts_dict_after`).
    let mut fields = dict.children(table.id, Kind::Field);
    fields.sort_by_key(|f| std::cmp::Reverse(f.record()));
    let mut files = dict.children(table.id, Kind::IndexFile);
    files.sort_by_key(|f| f.id);
    let mut keys = dict.children(table.id, Kind::Index);
    keys.sort_by_key(|k| std::cmp::Reverse(k.record()));
    let used = 2 * fields.len() + keys.len();
    let next = read_next_id(&add_path)? + used as u32;

    // The index files stay in their slots; the others take the Key objects
    // last to first, then the fields (`fixtures/sql/spddalterfail2` and
    // `spddalterfail3`).
    let file_slots: Vec<u32> = files.iter().map(|f| f.record()).collect();
    let mut slots: Vec<u32> = keys
        .iter()
        .map(|k| k.record())
        .chain(fields.iter().map(|f| f.record()))
        .chain(file_slots.iter().copied())
        .collect();
    slots.sort_unstable();
    let mut stream = keys
        .iter()
        .map(|k| k.record())
        .chain(fields.iter().map(|f| f.record()));
    let order: Vec<u32> = slots
        .iter()
        .map(|slot| {
            if file_slots.contains(slot) {
                Some(*slot)
            } else {
                stream.next()
            }
        })
        .collect::<Option<_>>()
        .ok_or(Error::CorruptTable)?;
    let freeing: Vec<u32> = files
        .iter()
        .flat_map(|file| {
            keys.iter()
                .filter(|k| k.info1 == Some(file.id as i32))
                .map(|k| k.record())
                .collect::<Vec<_>>()
        })
        .collect();
    drop(dict);
    {
        let mut file = open_dictionary_file(&add_path)?;
        let memo_path = dd::memo_path(&add_path);
        let mut records = order
            .iter()
            .map(|&slot| file.record(slot).map_err(|_| Error::CorruptTable))
            .collect::<Result<Vec<_>, _>>()?;
        // A Key object whose properties go on in the memo file (a full-text
        // order's, with its word lists) is made anew: the blocks of all of
        // them are freed, then each takes its new one, a file's orders at a
        // time, last to first (`fixtures/write/restructure_fts_dict_after`).
        let mut released = Vec::new();
        for source in freeing {
            let at = order
                .iter()
                .position(|&slot| slot == source)
                .ok_or(Error::CorruptTable)?;
            if dd::create::has_more_properties(&records[at]) {
                let slots = dd::create::release_properties(&mut records[at], &memo_path)
                    .map_err(|_| Error::CorruptTable)?;
                released.push((at, slots));
            }
        }
        for (at, slots) in released {
            dd::create::store_properties(&mut records[at], &memo_path, slots, |_| {})
                .map_err(|_| Error::CorruptTable)?;
        }
        for (slot, record) in slots.iter().zip(&records) {
            file.write(*slot, record).map_err(|_| Error::CorruptTable)?;
        }
    }
    dd::create::insert_objects(&add_path, &[], next).map_err(|_| Error::CorruptTable)?;
    rebuild_dictionary(session, &conn, &add_path)
}

/// Removes an index file from a table in the dictionary (`AdsDDRemoveIndexFile`):
/// soft-deletes the Index object and its Keys, and with `delete_file` removes
/// the file itself.
pub fn remove_index_file_api(
    session: &Session,
    connection: Handle,
    table_name: &str,
    index_file_name: &str,
    delete_file: bool,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let folder = dictionary_folder(&add_path).to_path_buf();
    let dict = open_dict(&add_path)?;
    let table = find_named_api(&dict, Kind::Table, "table", table_name)?;
    super::rights::require(&conn, 1, table_name, super::rights::ALTER, "")?;
    let Some(index) = dict.child(table.id, Kind::IndexFile, index_file_name) else {
        return Err(not_valid("The index file is not part of the database."));
    };

    // The index file first, then its keys (those whose Info1 is its id).
    let mut slots = vec![index.record()];
    slots.extend(
        dict.children(table.id, Kind::Index)
            .iter()
            .filter(|k| k.info1 == Some(index.id as i32))
            .map(|o| o.record()),
    );
    let file = delete_file
        .then(|| index.text(dd::index_file::RELATIVE_PATH))
        .flatten()
        .map(|p| folder.join(strip_relative(&p)));

    delete_records(session, &conn, &add_path, &slots)?;
    if let Some(file) = file {
        let _ = std::fs::remove_file(file);
    }
    Ok(())
}

/// Deletes one index order from a table in the dictionary (`AdsDDDeleteIndex`):
/// soft-deletes the Key object, and if it was the last order in its index file,
/// the Index object too (the file's records only; the file stays on disk).
pub fn delete_index_api(
    session: &Session,
    connection: Handle,
    table_name: &str,
    index_name: &str,
) -> Result<(), Error> {
    delete_index(session, connection, table_name, index_name, true)
}

/// [`delete_index_api`] where `from_file` says whether the order leaves its
/// index file too (`DROP INDEX` took it out already).
pub(crate) fn delete_index(
    session: &Session,
    connection: Handle,
    table_name: &str,
    index_name: &str,
    from_file: bool,
) -> Result<(), Error> {
    use tflip_storage::dd::Kind;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let table = find_named_api(&dict, Kind::Table, "index", table_name)?;
    let key = find_child_api(&dict, table.id, Kind::Index, "index", index_name)?;
    super::rights::require(
        &conn,
        1,
        table_name,
        super::rights::ALTER,
        "Permission to modify this table not granted.",
    )?;
    let index_file = key.info1;
    let path = index_file_of(session, connection, table_name, index_name);
    // Whether this is the only order in its index file.
    let siblings = dict
        .children(table.id, Kind::Index)
        .into_iter()
        .filter(|k| k.info1 == index_file)
        .count();
    // The engine deletes the index-file record first (if it empties), then the
    // key.
    let index_slot = (siblings == 1)
        .then(|| {
            index_file
                .and_then(|id| dict.object(id as u32))
                .map(|o| o.record())
        })
        .flatten();
    let mut slots: Vec<u32> = index_slot.into_iter().collect();
    slots.push(key.record());
    drop(dict);
    if from_file && let Some(path) = path {
        // The order leaves its file, which goes with its last order
        // (`fixtures/sql/ddapirights`).
        let _ =
            crate::ddl::drop_order_from_file(session, connection, table_name, &path, index_name);
    }
    delete_records(session, &conn, &add_path, &slots)
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn structural_writes_match_the_engine() {
        let session = Session::new();
        let fx = ops_fixtures();
        let base =
            tflip_storage::testing::temp_root().join(format!("tflip-ddops-{}", std::process::id()));
        type Apply = Box<dyn Fn(&Session, Handle)>;
        let cases: Vec<(&str, Apply)> = vec![
            (
                "removeview",
                Box::new(|s, c| remove_view_api(s, c, "v").unwrap()),
            ),
            (
                "removetable",
                Box::new(|s, c| remove_table_api(s, c, "t", false).unwrap()),
            ),
            (
                "deleteindex",
                Box::new(|s, c| delete_index_api(s, c, "t", "BY_ID").unwrap()),
            ),
            (
                "removeindexfile",
                Box::new(|s, c| remove_index_file_api(s, c, "t", "t.adi", false).unwrap()),
            ),
            (
                "rename",
                Box::new(|s, c| rename_object_api(s, c, "v", "vv", 6, 0).unwrap()),
            ),
        ];
        for (name, apply) in cases {
            let dir = base.join(name);
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let connect = build_ops_base(&session, &dir);
            apply(&session, connect);
            let ours = std::fs::read(dir.join("db.add")).unwrap();
            let golden = std::fs::read(fx.join(format!("{name}.add"))).unwrap();
            assert_eq!(mask_login(ours), mask_login(golden), "{name}: .add");
            assert_eq!(
                std::fs::read(dir.join("db.ai")).unwrap(),
                std::fs::read(fx.join(format!("{name}.ai"))).unwrap(),
                "{name}: .ai"
            );
            connection::disconnect(&session, connect).ok();
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
