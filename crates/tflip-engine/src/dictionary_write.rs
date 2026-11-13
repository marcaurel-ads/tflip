//! The dictionary-write functions behind the `AdsDD*` API: creating a data
//! dictionary and adding, removing, renaming and setting properties on its
//! objects, writing the files as the original engine writes them (byte for
//! byte; see [`tflip_storage::dd`] and `fixtures/dictionary`). The SQL DDL on
//! free tables is in [`crate::ddl`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tflip_sql::SqlError;
use tflip_storage::create;
use tflip_storage::dd::create::{INFO1, INFO2, NAME};

use crate::ace_error;
use crate::handle::Handle;
use crate::table::{self, TableType};
use crate::{Error, Session, connection, index, lock};

#[cfg(test)]
mod aes_convert;
#[cfg(test)]
mod aes_replay;
pub(crate) mod convert;
mod created;
mod deploy;
pub(crate) use deploy::build_restored_orders;
mod functions;
mod integrity;
mod links;
mod moves;
mod objects;
mod permissions;
mod properties;
#[cfg(test)]
mod rc4_replay;
pub(crate) mod rc4_tables;
mod removal;
mod replication;
pub(crate) mod rights;
mod tables;
#[cfg(test)]
mod test_support;
mod triggers;
mod users;
#[cfg(test)]
mod vfp_aes_replay;
mod views_procedures;
pub(crate) use created::{Created, add as add_created};
pub(crate) use deploy::auto_create;
pub use deploy::deploy_database_api;
pub(crate) use functions::{function_ddl, function_or_package};
pub use integrity::*;
pub use links::*;
pub use moves::*;
pub(crate) use objects::{
    create_database, create_procedure, create_view, drop_procedure, drop_view, name_taken,
};
pub use permissions::sql_grant;
pub use properties::*;
pub use removal::*;
pub use replication::*;
pub use tables::*;
pub use triggers::*;
pub use users::*;
pub use views_procedures::*;

/// The login salts a new dictionary needs. The engine fills them at random;
/// tflip only needs them to vary (it re-derives what it checks from the stored
/// salts), so they come from the clock, the process and a counter, spread over
/// the bytes with SplitMix64.
pub(crate) fn login_salts() -> tflip_storage::dd::create::NewDictRandom {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mut state = nanos
        ^ (u64::from(std::process::id()) << 32)
        ^ COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let mut fill = || {
        let mut bytes = [0u8; 20];
        for chunk in bytes.chunks_mut(8) {
            let n = next().to_le_bytes();
            chunk.copy_from_slice(&n[..chunk.len()]);
        }
        bytes
    };
    tflip_storage::dd::create::NewDictRandom {
        secret_first: fill(),
        secret_last: fill(),
        slot0: fill(),
        slot4: fill(),
    }
}

/// A string as the dictionary stores it: its bytes with a trailing NUL.
pub(crate) fn nul_bytes(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

/// 5132 with the description every refusal of a name has.
pub(crate) fn not_valid(detail: &str) -> Error {
    ace_error::sql(5132, detail)
}

/// 5132, the dictionary object name `name` is not valid (a reserved, empty or
/// already-used name), as the original engine answers `AdsDDCreateUserGroup`
/// and `AdsDDCreateUser` for a name it will not take.
pub(super) fn invalid_object_name(name: &str) -> Error {
    ace_error::sql(
        5132,
        &format!("The object name is reserved or already in use. Object name: {name}"),
    )
}

/// Raises the dictionary's version to `target` (with its version string), as the
/// original engine does when a feature that needs a newer engine is first used:
/// the Database object's `Info1` and `VERSION` slot go to `target` and its
/// version-string slot to `version_string`. A no-op once the version is already
/// at least `target`, so the raise is monotonic (`false`). The login-secret slots
/// are left as they are.
pub(super) fn raise_dictionary_version(
    add_path: &Path,
    target: i32,
    version_string: &[u8],
) -> Result<bool, Error> {
    use tflip_storage::dd::create;

    const VERSION_SLOT: usize = 11;
    const VERSION_STRING_SLOT: usize = 17;
    let mut file = open_dictionary_file(add_path)?;
    let mut record = file.record(1).map_err(|_| Error::CorruptTable)?; // Database is record 1.
    if i32::from_le_bytes(record[INFO1..INFO1 + 4].try_into().unwrap()) >= target {
        return Ok(false);
    }
    file.write_part(1, INFO1, &target.to_le_bytes())
        .map_err(|_| Error::CorruptTable)?;
    let (at, len) = (create::PROPERTY, create::PROPERTY_LENGTH);
    let blob = &mut record[at..at + len];
    create::set_property_slot(blob, VERSION_SLOT, &(target as u32).to_le_bytes())
        .map_err(|_| Error::CorruptTable)?;
    create::set_property_slot(blob, VERSION_STRING_SLOT, version_string)
        .map_err(|_| Error::CorruptTable)?;
    let blob = record[at..at + len].to_vec();
    file.write_part(1, at, &blob)
        .map_err(|_| Error::CorruptTable)?;
    Ok(true)
}

/// Raises the version when the first permission is granted: to 3 (`7.1`), as
/// permissions need a newer engine; whether it was raised.
pub(super) fn raise_permission_version(add_path: &Path) -> Result<bool, Error> {
    raise_dictionary_version(add_path, 3, b"7.1\0")
}

/// Raises the version when a trigger is created: to 4 (`8.1`), as triggers need a
/// newer engine still.
pub(super) fn raise_trigger_version(add_path: &Path) -> Result<(), Error> {
    raise_dictionary_version(add_path, 4, b"8.1\0").map(|_| ())
}

/// Sets (`Some`) or drops (`None`) property slots of the object at record `slot`,
/// then rebuilds the index and reloads. The object's other properties are
/// preserved.
pub(crate) fn write_object_slots(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    slot: u32,
    slots: &[(usize, Option<&[u8]>)],
) -> Result<(), Error> {
    write_object(session, conn, add_path, slot, slots, None)
}

/// Sets property slot `index` of the object at record `slot` to `value` for a
/// writer with no connection of its own to the dictionary: the server, that
/// locks a user out after failed logins.
pub(crate) fn write_slot_unconnected(
    session: &Session,
    add_path: &Path,
    slot: u32,
    index: usize,
    value: &[u8],
) -> Result<(), Error> {
    {
        let mut file = open_dictionary_file(add_path)?;
        edit_object_properties(&mut file, add_path, slot, |properties| {
            set_slot(properties, index, Some(value));
        })?;
    }
    rebuild_index(session, add_path)
}

/// [`write_object_slots`], and the object's `Info1` too if given, in the one
/// write of its record.
pub(super) fn write_object(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    slot: u32,
    slots: &[(usize, Option<&[u8]>)],
    info1: Option<i32>,
) -> Result<(), Error> {
    {
        let mut file = open_dictionary_file(add_path)?;
        edit_object_properties(&mut file, add_path, slot, |properties| {
            for &(index, value) in slots {
                set_slot(properties, index, value);
            }
        })?;
        if let Some(info1) = info1 {
            file.write_part(slot, INFO1, &info1.to_le_bytes())
                .map_err(|_| Error::CorruptTable)?;
        }
    }
    rebuild_dictionary(session, conn, add_path)
}

/// Writes the name of the object at record `slot`: the 200-byte Object Name
/// field, space-padded.
pub(crate) fn write_object_name(add_path: &Path, slot: u32, name: &str) -> Result<(), Error> {
    let mut field = [b' '; 200];
    let bytes = name.as_bytes();
    let n = bytes.len().min(200);
    field[..n].copy_from_slice(&bytes[..n]);
    open_dictionary_file(add_path)?
        .write_part(slot, NAME, &field)
        .map_err(|_| Error::CorruptTable)
}

/// Takes the next object id, as the original does for an object it then
/// refuses; with `blank` it also leaves a blank, free slot in the dictionary
/// (`fixtures/sql/ddprocs`).
pub(crate) fn bump_next_id(add_path: &Path, blank: bool) -> Result<(), Error> {
    let next = read_next_id(add_path)?;
    let mut file = open_dictionary_file(add_path)?;
    if blank {
        let mut record = vec![0u8; tflip_storage::dd::create::RECORD_LENGTH];
        record[0] = tflip_storage::adt::ACTIVE;
        // A NULL parent and NULL info numbers, as the engine writes them.
        for at in [9, INFO1, INFO2] {
            record[at..at + 4].copy_from_slice(&0x8000_0000u32.to_le_bytes());
        }
        let slot = file.insert(&record).map_err(|_| Error::CorruptTable)?;
        file.delete(slot, None).map_err(|_| Error::CorruptTable)?;
    }
    file.set_next_autoinc(next + 1)
        .map_err(|_| Error::CorruptTable)
}

/// Rebuilds the dictionary's `.ai` index over its current (non-deleted)
/// objects and reloads the connection's dictionary, after its records were
/// changed in `add_path`.
pub(crate) fn rebuild_dictionary(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
) -> Result<(), Error> {
    rebuild_index(session, add_path)?;
    conn.reload_database()
}

/// [`rebuild_dictionary`] for a change made with no connection of the
/// writer's to reload.
pub(crate) fn rebuild_index(session: &Session, add_path: &Path) -> Result<(), Error> {
    let table = Arc::new(table::dictionary(session, add_path)?);
    let weights = table
        .session_settings()
        .collations()
        .ansi()
        .map(|c| c.weights);
    let collation = create::collation_crc(weights.as_ref());
    let ai_path = add_path.with_extension("ai");
    if index::update_dictionary_index(&table, &ai_path)? {
        return Ok(());
    }
    std::fs::remove_file(&ai_path).ok();
    index::build_dictionary_index(&table, &ai_path, collation)?;
    Ok(())
}

/// The dictionary at `add_path`, read.
pub(crate) fn open_dict(add_path: &Path) -> Result<tflip_storage::dd::Dictionary, Error> {
    tflip_storage::dd::Dictionary::open(add_path).map_err(|_| Error::CorruptTable)
}

/// The folder the dictionary at `add_path` is in, which its tables' paths are
/// relative to.
pub(crate) fn dictionary_folder(add_path: &Path) -> &Path {
    add_path.parent().unwrap_or_else(|| Path::new("."))
}

/// The object of the kind `kind` named `name`, or 5132 naming it as `what`.
pub(super) fn find_named<'a>(
    dict: &'a tflip_storage::dd::Dictionary,
    kind: tflip_storage::dd::Kind,
    what: &str,
    name: &str,
) -> Result<&'a tflip_storage::dd::Object, Error> {
    dict.named(kind, name)
        .ok_or_else(|| no_such_object(what, name))
}

/// [`find_named`] with the words the `AdsDD` calls give a name that is none:
/// "There is no such named <what> in the data dictionary."
/// (`fixtures/sql/ddapirights`).
pub(super) fn find_named_api<'a>(
    dict: &'a tflip_storage::dd::Dictionary,
    kind: tflip_storage::dd::Kind,
    what: &str,
    name: &str,
) -> Result<&'a tflip_storage::dd::Object, Error> {
    dict.named(kind, name).ok_or_else(|| not_named(what))
}

/// [`find_child`] with the words of [`find_named_api`].
pub(super) fn find_child_api<'a>(
    dict: &'a tflip_storage::dd::Dictionary,
    parent: u32,
    kind: tflip_storage::dd::Kind,
    what: &str,
    name: &str,
) -> Result<&'a tflip_storage::dd::Object, Error> {
    dict.child(parent, kind, name)
        .ok_or_else(|| not_named(what))
}

/// 5132, "There is no such named <what> in the data dictionary."
pub(super) fn not_named(what: &str) -> Error {
    not_valid(&format!(
        "There is no such named {what} in the data dictionary."
    ))
}

/// The object of the kind `kind` named `name` under the object `parent`, or
/// 5132 naming it as `what`.
pub(super) fn find_child<'a>(
    dict: &'a tflip_storage::dd::Dictionary,
    parent: u32,
    kind: tflip_storage::dd::Kind,
    what: &str,
    name: &str,
) -> Result<&'a tflip_storage::dd::Object, Error> {
    dict.child(parent, kind, name)
        .ok_or_else(|| no_such_object(what, name))
}

/// The dictionary's table at `add_path`, to write to. One that encrypts its
/// records with AES opens once a connection of the process gave its
/// dictionary password ([`Error::Encrypted`] in storage, 7068 here).
pub(crate) fn open_dictionary_file(
    add_path: &Path,
) -> Result<tflip_storage::dd::DictionaryFile, Error> {
    tflip_storage::dd::DictionaryFile::open(add_path).map_err(|error| match error {
        tflip_storage::table::Error::Encrypted => Error::DictionaryEncrypted,
        _ => Error::CorruptTable,
    })
}

/// The data dictionary `connection` is to, as a writable connection and the
/// path of its `.add`; [`Error::InvalidConnection`] if it is not a dictionary.
/// The dictionary key `K` a writer enciphers with: the one the connection's
/// login recovered, so a dictionary with an administrator password is
/// written by whoever logged in to it.
pub(crate) fn connection_key(
    conn: &connection::Connection,
    dict: &tflip_storage::dd::Dictionary,
) -> Option<Vec<u8>> {
    conn.with_database_opt(|db| db.key())
        .flatten()
        .or_else(|| dict.dictionary_key(b""))
}

pub(crate) fn dictionary_connection(
    session: &Session,
    connection: Handle,
) -> Result<(std::sync::Arc<connection::Connection>, PathBuf), Error> {
    let conn = connection::get(session, connection)?;
    if !conn.dictionary {
        return Err(Error::InvalidConnection);
    }
    let add_path = conn.path.clone();
    Ok((conn, add_path))
}

/// Soft-deletes the dictionary records at `slots` (1-based, in the engine's
/// deletion order), threading the deleted-record free list as the engine does,
/// then rebuilds the index and reloads. The Database object is never touched.
/// Each record's memo blocks (`More Property`, then the comment) are freed
/// as its turn comes.
pub(super) fn delete_records(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    slots: &[u32],
) -> Result<(), Error> {
    delete_records_as(session, conn, add_path, slots, true)
}

/// [`delete_records`] for records whose memo blocks the records that replace
/// them refer to still.
pub(super) fn delete_records_keeping_memos(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    slots: &[u32],
) -> Result<(), Error> {
    delete_records_as(session, conn, add_path, slots, false)
}

fn delete_records_as(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    slots: &[u32],
    free_memos: bool,
) -> Result<(), Error> {
    {
        let mut file = open_dictionary_file(add_path)?;
        for &slot in slots {
            if free_memos {
                tflip_storage::dd::create::free_object_memos(add_path, slot)
                    .map_err(|_| Error::CorruptTable)?;
            }
            file.delete(slot, None).map_err(|_| Error::CorruptTable)?;
        }
    }
    rebuild_dictionary(session, conn, add_path)
}

/// The records of the Permission objects on the object `id`, which go with
/// it (`fixtures/sql/ddviews`, `ddprocs`).
pub(super) fn permission_records(dict: &tflip_storage::dd::Dictionary, id: u32) -> Vec<u32> {
    dict.all(tflip_storage::dd::Kind::Permission)
        .into_iter()
        .filter(|p| p.info1 == i32::try_from(id).ok())
        .map(|p| p.record())
        .collect()
}

/// 5004, the dictionary object `name` (of `what`, for the message) does not
/// exist.
pub(super) fn no_such_object(what: &str, name: &str) -> Error {
    ace_error::sql(
        5132,
        &format!("There is no such {what} in the data dictionary. Object name: {name}"),
    )
}
