//! Tables of a dictionary of RC4 tables that has a table password: the
//! dictionary's `ENCRYPT_NEW_TABLE` and `ENCRYPT_INDEXES` properties, which
//! the original allows only with the password set, and what they make of the
//! tables it creates (`fixtures/aes/rc4tp`, made by
//! `tools/ace/crypto/rc4tp/run.sh`).

use tflip_storage::crypto::Scheme;
use tflip_storage::dd::{self, Dictionary};

use crate::connection::Connection;

/// Whether the dictionary has a table password: the length byte of the
/// Database object's login slot 1, once deobfuscated, is not 0.
pub(crate) fn has_table_password(dictionary: &Dictionary) -> bool {
    let Some(database) = dictionary.database() else {
        return false;
    };
    let mut slot = database.slot(1).unwrap_or(&[]).to_vec();
    tflip_storage::crypto::obfuscate(dictionary.scheme(), &mut slot, database.record(), 1);
    slot.first().is_some_and(|&length| length != 0)
}

/// The table password of the dictionary `connection` is to, if it is one of
/// RC4 tables with the password set and the login password releases it.
pub(crate) fn table_key(connection: &Connection) -> Option<Vec<u8>> {
    connection
        .with_database_opt(|db| match db.scheme() {
            Scheme::Rc4 => db.dictionary().table_password(db.password()),
            _ => None,
        })
        .flatten()
        .filter(|password| !password.is_empty())
}

/// The table password new tables are encrypted with, if the dictionary
/// encrypts them (`ENCRYPT_NEW_TABLE` on, and the password set).
pub(crate) fn new_table_key(connection: &Connection) -> Option<Vec<u8>> {
    let on = connection
        .with_database_opt(|db| {
            db.dictionary()
                .database()
                .is_some_and(|d| d.has(dd::database::ENCRYPT_NEW_TABLE))
        })
        .unwrap_or(false);
    table_key(connection).filter(|_| on)
}

/// Whether the dictionary is one of AES tables that encrypts the ones it
/// makes (`ENCRYPT_NEW_TABLE`), which takes no Clipper or FoxPro table
/// (7168, `fixtures/aes/rc4tp`, `y5`).
pub(crate) fn refuses_dbf_without_strong_encryption(
    connection: &Connection,
    table_type: crate::table::TableType,
) -> bool {
    use crate::table::TableType;
    matches!(table_type, TableType::Ntx | TableType::Cdx)
        && connection
            .with_database_opt(|db| {
                db.scheme() != Scheme::Rc4
                    && db
                        .dictionary()
                        .database()
                        .is_some_and(|d| d.has(dd::database::ENCRYPT_NEW_TABLE))
            })
            .unwrap_or(false)
}

/// Whether the dictionary encrypts the index files of its encrypted tables
/// (`ENCRYPT_INDEXES`).
pub(crate) fn encrypts_indexes(connection: &Connection) -> bool {
    connection
        .with_database_opt(|db| {
            db.scheme() == Scheme::Rc4
                && db
                    .dictionary()
                    .database()
                    .is_some_and(|d| d.has(dd::database::ENCRYPT_INDEXES))
        })
        .unwrap_or(false)
}

/// Opens the table at `path` of the dictionary `connection` is to as a
/// transient table, deciphered with the password its tables share: the
/// dictionary password of an AES dictionary, the table password of an RC4
/// one that has it.
pub(crate) fn transient_in_dictionary(
    session: &crate::Session,
    connection: &Connection,
    path: &std::path::Path,
    format: tflip_storage::table::Format,
) -> Result<crate::table::Table, crate::Error> {
    let password = connection
        .with_database_opt(|db| match db.scheme() {
            Scheme::Rc4 => db.dictionary().table_password(db.password()),
            _ => db.dd_password().map(<[u8]>::to_vec),
        })
        .flatten()
        .filter(|password| !password.is_empty());
    match password {
        Some(password) => crate::table::transient_deciphered(session, path, format, &password),
        None => crate::table::transient(session, path, format),
    }
}

/// `sp_ModifyDatabase` and `AdsDDSetDatabaseProperty` of `ENCRYPT_NEW_TABLE`
/// (`slot` is [`dd::database::ENCRYPT_NEW_TABLE`]) and `ENCRYPT_INDEXES`:
/// the flag on or off in the Database object `record`; one of RC4 tables is
/// not set on while there is no table password, 5162.
pub(crate) fn set_flag(
    session: &crate::Session,
    connection: &Connection,
    add_path: &std::path::Path,
    slot: usize,
    on: bool,
) -> Result<(), crate::Error> {
    let dictionary = super::open_dict(add_path)?;
    if on && dictionary.scheme() == Scheme::Rc4 && !has_table_password(&dictionary) {
        return Err(crate::ace_error::sql(5162, ""));
    }
    let record = dictionary
        .database()
        .ok_or(crate::Error::CorruptTable)?
        .record();
    drop(dictionary);
    super::set_object_presence(session, connection, add_path, record, slot, on)
}
