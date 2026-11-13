//! A table SQL `CREATE TABLE` makes on a dictionary connection, joining the
//! dictionary as the original engine adds it — probed black-box with
//! `tools/ace/sql/ddtables.txt` (`fixtures/sql/ddtables`) and, for DBF tables,
//! `ddbf.txt` (`fixtures/sql/ddbf`). Its objects are those `AdsDDAddTable`
//! writes, with the field rules and memo block size the statement gives and
//! its primary key's order, stored in the slots of deleted objects first.

use std::sync::Arc;

use tflip_sql::ast::FieldRules;
use tflip_storage::dbf::Flavour;
use tflip_storage::dd::{self, Kind, create::NewObject};
use tflip_storage::table::Format;

use super::{
    append_index_records, dictionary_connection, dictionary_folder, open_dict,
    open_dictionary_file, read_next_id, rebuild_dictionary,
};
use crate::handle::Handle;
use crate::table::{self, TableType};
use crate::{Error, Session};

/// A table `CREATE TABLE` made in a dictionary's folder, to add to it.
pub(crate) struct Created<'a> {
    /// The table's name in the dictionary.
    pub name: &'a str,
    /// Its type, and its character type (`ADS_OEM` is kept by a DBF table
    /// alone).
    pub table_type: TableType,
    pub char_type: u16,
    /// Its file, relative to the dictionary's folder.
    pub file: &'a str,
    /// Each field's rules, in field order.
    pub rules: Vec<&'a FieldRules>,
    /// `MEMOBLOCKSIZE`, if the statement gives it.
    pub block_size: Option<u16>,
    /// The index file holding the primary key's order, the order alone.
    pub primary_key: Option<&'a str>,
    /// `IGNORE TRANSACTIONS`: the table is transaction-free.
    pub trans_free: bool,
}

/// Adds the table `created` describes to the dictionary `connection` is to:
/// a Table object (its primary key's Key object id in `Info1`, or in a
/// property for a DBF table), a Field
/// object per field in reverse order with its rules, and the primary key's
/// Index and Key objects; then binds the table's file and rebuilds the
/// dictionary's index. A DBF table's file is left as it is; it has nowhere to
/// say it is in a dictionary.
///
/// A user who is not the administrator is given its rights on the table
/// before the primary key's objects are added, and `true` says it was
/// (`fixtures/sql/ddadminpk`).
pub(crate) fn add(session: &Session, connection: Handle, created: &Created) -> Result<bool, Error> {
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let folder = dictionary_folder(&add_path).to_path_buf();
    let table_path = folder.join(created.file);
    let format = match created.table_type {
        TableType::Ntx => Format::Dbf(Flavour::Clipper),
        TableType::Cdx => Format::Dbf(Flavour::FoxPro),
        TableType::Vfp => Format::Dbf(Flavour::Visual),
        TableType::Adt | TableType::Database => Format::Adt,
    };
    let source =
        Arc::new(table::transient(session, &table_path, format).map_err(|_| Error::CorruptTable)?);
    let fields = source.fields();
    let memo_path = add_path.with_extension("am");

    const DATABASE_ID: i32 = 1;
    let mut next_id = read_next_id(&add_path)?;
    let table_id = next_id;
    next_id += 1;

    let relative = format!(".\\{}\0", created.file);
    let field_count = (fields.len() as u16).to_le_bytes();
    let block_size = created.block_size.map(u16::to_le_bytes);
    let mut table_slots: Vec<(usize, &[u8])> = vec![
        (dd::table::RELATIVE_PATH, relative.as_bytes()),
        (dd::table::NO_AUTO_CREATE, &[]),
        (dd::table::FIELD_COUNT, &field_count),
    ];
    if let Some(block_size) = &block_size {
        table_slots.push((dd::table::MEMO_BLOCK_SIZE, block_size));
    }
    // A dictionary of AES tables that encrypts new ones arms it (see below),
    // and its object says so.
    let aes_encrypt = matches!(created.table_type, TableType::Adt | TableType::Vfp)
        && conn
            .with_database_opt(|db| {
                db.scheme() != tflip_storage::crypto::Scheme::Rc4
                    && db
                        .dictionary()
                        .database()
                        .is_some_and(|d| d.has(dd::database::ENCRYPT_NEW_TABLE))
            })
            .unwrap_or(false);
    // A dictionary of RC4 tables with a table password does the same with
    // that password (`fixtures/aes/rc4tp`).
    let rc4_key = super::rc4_tables::new_table_key(&conn);
    let encrypt = aes_encrypt || rc4_key.is_some();
    let table_record = |slots: &[(usize, &[u8])]| {
        dd::create::object_record(&NewObject {
            id: table_id,
            parent: DATABASE_ID,
            kind: "Table",
            name: created.name,
            info1: crate::dictionary::table_type_info(created.table_type),
            info2: crate::dictionary::table_info2(created.table_type, created.char_type)
                | if created.trans_free {
                    dd::table::flags::TRANSACTION_FREE as u32
                } else {
                    0
                }
                | if encrypt {
                    dd::table::flags::ENCRYPTED as u32
                } else {
                    0
                },
            slots,
        })
    };
    let mut records = vec![table_record(&table_slots)];

    // The Field objects are stored last column first, but their continuations
    // in the memo file are made first column first
    // (`fixtures/sql/spddlongalter2`).
    let field_ids = next_id;
    let mut field_records = Vec::with_capacity(fields.len());
    for (position, field) in fields.iter().enumerate() {
        let definition = [
            field.field_type.code().to_le_bytes(),
            super::tables::recorded_length(field).to_le_bytes(),
            field.decimals().to_le_bytes(),
        ]
        .concat();
        let options = super::dbf_field_options(field).to_le_bytes();
        let rules = created
            .rules
            .get(position)
            .copied()
            .cloned()
            .unwrap_or_default();
        let text = |value: &Option<String>| value.as_ref().map(|v| format!("{v}\0"));
        let (default, minimum) = (text(&rules.default), text(&rules.minimum));
        let (maximum, message) = (text(&rules.maximum), text(&rules.message));
        let mut slots: Vec<(usize, &[u8])> = Vec::new();
        if let Some(default) = &default {
            slots.push((dd::field::DEFAULT_VALUE, default.as_bytes()));
        }
        if rules.not_null {
            slots.push((dd::field::NOT_NULL, &[]));
        }
        if let Some(minimum) = &minimum {
            slots.push((dd::field::MIN_VALUE, minimum.as_bytes()));
        }
        if let Some(maximum) = &maximum {
            slots.push((dd::field::MAX_VALUE, maximum.as_bytes()));
        }
        if let Some(message) = &message {
            slots.push((dd::field::VALIDATION_MSG, message.as_bytes()));
        }
        slots.push((dd::field::DEFINITION, &definition));
        slots.push((dd::field::OPTIONS, &options));
        let object = NewObject {
            id: field_ids + (fields.len() - 1 - position) as u32,
            parent: table_id as i32,
            kind: "Field",
            name: &field.name,
            info1: position as i32 + 1,
            info2: 0x8000_0000,
            slots: &slots,
        };
        // The original sets a column's properties one at a time: its
        // definition and options, then its rules, the maximum before the
        // minimum (`fixtures/sql/spddlong`, `spddlongalter3`).
        field_records.push(
            dd::create::object_record_stepwise(
                &object,
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
            .map_err(|_| Error::CorruptTable)?,
        );
    }
    next_id += fields.len() as u32;
    records.extend(field_records.into_iter().rev());

    let keyed_later = created.primary_key.is_some()
        && conn
            .with_database_opt(|db| db.creator())
            .flatten()
            .is_some();
    if let Some(index_file) = created.primary_key.filter(|_| !keyed_later) {
        append_index_records(
            &source,
            table_id,
            &mut next_id,
            &folder,
            &memo_path,
            index_file,
            &mut records,
        )?;
        // The table names its primary key: the one order's Key object, the
        // last made, in its `Info1` if it is an ADT table, else (`Info1` is
        // its type) in a property.
        let key = (next_id - 1).to_le_bytes();
        match created.table_type {
            TableType::Adt | TableType::Database => {
                records[0][dd::create::INFO1..dd::create::INFO1 + 4].copy_from_slice(&key);
            }
            _ => {
                table_slots.push((dd::table::PRIMARY_KEY, &key));
                records[0] = table_record(&table_slots);
            }
        }
    }
    dd::create::insert_objects(&add_path, &records, next_id).map_err(|_| Error::CorruptTable)?;
    if let (true, Some(index_file)) = (keyed_later, created.primary_key) {
        rebuild_dictionary(session, &conn, &add_path)?;
        super::rights::grant_creator(session, connection, 1, created.name)?;
        let mut next_id = read_next_id(&add_path)?;
        let mut keyed = Vec::new();
        append_index_records(
            &source,
            table_id,
            &mut next_id,
            &folder,
            &memo_path,
            index_file,
            &mut keyed,
        )?;
        dd::create::insert_objects(&add_path, &keyed, next_id).map_err(|_| Error::CorruptTable)?;
        let record = open_dict(&add_path)?
            .named(Kind::Table, created.name)
            .ok_or(Error::CorruptTable)?
            .record();
        let key = (next_id - 1).to_le_bytes();
        match created.table_type {
            TableType::Adt | TableType::Database => open_dictionary_file(&add_path)?
                .write_part(record, dd::create::INFO1, &key)
                .map_err(|_| Error::CorruptTable)?,
            _ => super::properties::set_object_slot(
                session,
                &conn,
                &add_path,
                record,
                dd::table::PRIMARY_KEY,
                &key,
            )?,
        }
    }
    drop(source);
    let dictionary_file = add_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if created.table_type == TableType::Adt {
        dd::create::bind_table(&table_path, table_id, &dictionary_file)
            .map_err(|_| Error::CorruptTable)?;
        if aes_encrypt {
            dd::create::arm_table(&table_path, &add_path).map_err(|_| Error::CorruptTable)?;
        }
        if let Some(key) = &rc4_key {
            dd::create::arm_table_rc4(&table_path, key).map_err(|_| Error::CorruptTable)?;
        }
    } else if aes_encrypt {
        // A Visual FoxPro table is laid out for AES by its creation, and armed
        // here (`fixtures/aes/rc4tp`, `v1`).
        tflip_storage::table::TableFile::open_for_writing(
            &table_path,
            Format::Dbf(Flavour::Visual),
        )
        .and_then(|mut file| file.encrypt_for_dictionary(&add_path, &[]))
        .map_err(|_| Error::CorruptTable)?;
    } else if let Some(key) = &rc4_key {
        let format = match created.table_type {
            TableType::Ntx => Format::Dbf(Flavour::Clipper),
            TableType::Vfp => Format::Dbf(Flavour::Visual),
            _ => Format::Dbf(Flavour::FoxPro),
        };
        tflip_storage::table::TableFile::open_for_writing(&table_path, format)
            .and_then(|mut file| file.encrypt_for_dictionary_rc4(key, &[]))
            .map_err(|_| Error::CorruptTable)?;
    }
    rebuild_dictionary(session, &conn, &add_path)?;
    Ok(keyed_later)
}
