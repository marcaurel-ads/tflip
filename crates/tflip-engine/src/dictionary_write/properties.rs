//! Object comments and the property setters.

use super::*;

/// The `ADS_DD_COMMENT` property id.
pub(super) const ADS_DD_COMMENT: u16 = 1;

/// The `ADS_DD_ADMIN_PASSWORD` property id.
const ADS_DD_ADMIN_PASSWORD: u16 = 101;

pub(super) const ADS_DD_USER_DEFINED_PROP: u16 = 3;

/// Sets the comment of the object at record `slot` to `comment`, as the engine
/// stores an object's comment (`ADS_DD_SetProperty` with `ADS_DD_COMMENT`): the
/// text goes into the dictionary memo file and its reference into the record's
/// Comment field; a comment it replaces has its memo block freed once the new
/// one is stored. Then the index is rebuilt and the connection reloaded.
pub(super) fn set_object_comment(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    slot: u32,
    comment: &[u8],
) -> Result<(), Error> {
    use tflip_storage::dd;

    {
        let mut file = open_dictionary_file(add_path)?;
        // set_comment writes the Comment field reference (8 bytes at 0x515); a
        // throwaway buffer carries it, then it is written back to the record.
        let mut field = vec![0u8; dd::create::RECORD_LENGTH];
        dd::create::set_comment(&mut field, &dd::memo_path(add_path), comment)
            .map_err(|_| Error::CorruptTable)?;
        let at = dd::create::COMMENT;
        let (old_block, old_length) =
            dd::create::comment_ref(add_path, slot).map_err(|_| Error::CorruptTable)?;
        file.write_part(slot, at, &field[at..at + 8])
            .map_err(|_| Error::CorruptTable)?;
        dd::create::free_comment(&dd::memo_path(add_path), old_block, old_length)
            .map_err(|_| Error::CorruptTable)?;
    }
    rebuild_dictionary(session, conn, add_path)
}

/// [`set_object_comment`] as the original does it for the comment of a
/// publication or a subscription: a comment of as many blocks as the old one
/// is written over it, any other is stored once the old one is freed
/// (`fixtures/sql/ddrepl2`).
pub(super) fn set_replication_comment(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    slot: u32,
    comment: &[u8],
) -> Result<(), Error> {
    use tflip_storage::dd;

    {
        let memo = dd::memo_path(add_path);
        let mut file = open_dictionary_file(add_path)?;
        let at = dd::create::COMMENT;
        let (old_block, old_length) =
            dd::create::comment_ref(add_path, slot).map_err(|_| Error::CorruptTable)?;
        if let Some(stored) = dd::create::rewrite_comment(&memo, old_block, old_length, comment)
            .map_err(|_| Error::CorruptTable)?
        {
            file.write_part(slot, at + 4, &stored.to_le_bytes())
                .map_err(|_| Error::CorruptTable)?;
            drop(file);
            return rebuild_dictionary(session, conn, add_path);
        }
        dd::create::free_comment(&memo, old_block, old_length).map_err(|_| Error::CorruptTable)?;
        let mut field = vec![0u8; dd::create::RECORD_LENGTH];
        dd::create::set_comment(&mut field, &memo, comment).map_err(|_| Error::CorruptTable)?;
        file.write_part(slot, at, &field[at..at + 8])
            .map_err(|_| Error::CorruptTable)?;
    }
    rebuild_dictionary(session, conn, add_path)
}

/// The comment text in a property value (`ADS_DD_COMMENT` is a NUL-terminated
/// string), without its terminator.
pub(crate) fn comment_value(value: &[u8]) -> &[u8] {
    &value[..value.iter().position(|&b| b == 0).unwrap_or(value.len())]
}

/// Encrypts the table `name` of the AES dictionary `connection` is to when
/// `on`, else decrypts it: the table's file is changed (see
/// [`tflip_storage::table::TableFile::encrypt_for_dictionary`]) and the
/// dictionary's Table object marked (`TABLE_ENCRYPTION`, `sp_EncryptTable`,
/// `sp_DecryptTable`; `fixtures/aes/dictwrite`, `08_encrypt`, `09_decrypt`).
/// A table that is in the state asked for is left as it is, except through
/// `strict`, the property's way, which refuses to encrypt an encrypted one.
pub(crate) fn set_table_encryption(
    session: &Session,
    connection: Handle,
    name: &str,
    on: bool,
    strict: bool,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let aes = conn
        .with_database_opt(|db| db.scheme() != tflip_storage::crypto::Scheme::Rc4)
        .unwrap_or(false);
    // A dictionary of RC4 tables keys them with its table password.
    let rc4_key = super::rc4_tables::table_key(&conn);
    if on && !aes && rc4_key.is_none() {
        return Err(ace_error::sql(5162, ""));
    }
    let (slot, encrypted) = {
        let dict = open_dict(&add_path)?;
        let table = find_named(&dict, Kind::Table, "table", name)?;
        let slot = table.record();
        let record = open_dictionary_file(&add_path)?
            .record(slot)
            .map_err(|_| Error::CorruptTable)?;
        let flags = i32::from_le_bytes(record[INFO2..INFO2 + 4].try_into().unwrap());
        (slot, flags & dd::table::flags::ENCRYPTED != 0)
    };
    if encrypted == on {
        return match (on, strict) {
            (true, true) => Err(Error::TableEncrypted.detailed("The table is encrypted.")),
            (false, true) if !aes => Err(ace_error::sql(5163, "")),
            _ => Ok(()),
        };
    }
    let settings = crate::table::TableSettings {
        options: crate::table::options::EXCLUSIVE,
        ..Default::default()
    };
    let indexes = conn
        .with_database_opt(|db| db.table(name))
        .flatten()
        .map(|bound| bound.index_files)
        .unwrap_or_default();
    // Only the dictionary's own say decides whether an RC4 table's index
    // files are enciphered.
    let rc4_indexes: Vec<_> = match (on, aes) {
        (true, false) if !super::rc4_tables::encrypts_indexes(&conn) => Vec::new(),
        (true, false) => indexes.clone(),
        (false, false) => indexes
            .iter()
            .filter(|index| tflip_storage::adi::is_aes(index))
            .cloned()
            .collect(),
        _ => Vec::new(),
    };
    if on && aes {
        lay_out_for_aes(session, connection, name)?;
    }
    let handle = crate::table::open(session, connection, name, settings)?;
    let changed = crate::table::get(session, handle).and_then(|table| match (on, aes) {
        (true, true) => table.encrypt_for_dictionary(&add_path, &indexes),
        (false, true) => table.decrypt_for_dictionary(&indexes),
        (true, false) => {
            table.encrypt_for_dictionary_rc4(&rc4_key.unwrap_or_default(), &rc4_indexes)
        }
        (false, false) => table.decrypt_for_dictionary_rc4(&rc4_indexes),
    });
    let _ = crate::table::close(session, handle);
    changed?;
    let bit = dd::table::flags::ENCRYPTED as u32;
    set_object_info2(
        session,
        &conn,
        &add_path,
        slot,
        bit,
        if on { bit } else { 0 },
    )
}

/// Lays the table `name` of an AES dictionary out for AES before it is
/// encrypted, if it is a Visual FoxPro table that is not: the original puts
/// the old files aside (`.BAK`) and writes the table anew with an IV in each
/// record and before each memo value; a Clipper or FoxPro table has no such
/// layout and is refused (`fixtures/aes/rc4tp`, `v11`).
fn lay_out_for_aes(session: &Session, connection: Handle, name: &str) -> Result<(), Error> {
    use crate::table::TableType;
    let Some((path, _, table_type)) = crate::table::open_info(
        session,
        connection,
        name,
        &crate::table::TableSettings::default(),
    ) else {
        return Ok(());
    };
    match table_type {
        TableType::Cdx | TableType::Ntx => {
            return Err(Error::RestructureFailed("Nothing to be done.".to_owned()));
        }
        TableType::Vfp => {}
        _ => return Ok(()),
    }
    let aside = |path: &std::path::Path| {
        let mut name = path.as_os_str().to_owned();
        name.push(".BAK");
        std::path::PathBuf::from(name)
    };
    let memo = tflip_storage::dbf::memo_path(&path, tflip_storage::memo::MemoKind::Fpt);
    let head = std::fs::read(&path).map_err(|_| Error::CorruptTable)?;
    if head.first().is_some_and(|&version| version >= 0xF8) || head.len() < 12 {
        return Ok(());
    }
    for file in [&path, &memo] {
        if file.exists() {
            std::fs::copy(file, aside(file)).map_err(|_| Error::FileCreate)?;
        }
    }
    crate::cache::evict_kind(crate::cache::Kind::Tables);
    tflip_storage::table::TableFile::lay_out_dbf_for_aes(&path).map_err(|_| Error::CorruptTable)
}

/// Whether the Table object of `name` says the table is encrypted.
pub(crate) fn table_marked_encrypted(
    session: &Session,
    connection: Handle,
    name: &str,
) -> Result<bool, Error> {
    use tflip_storage::dd::{self, Kind};
    let (_, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let table = find_named(&dict, Kind::Table, "table", name)?;
    let record = open_dictionary_file(&add_path)?
        .record(table.record())
        .map_err(|_| Error::CorruptTable)?;
    let flags = i32::from_le_bytes(record[INFO2..INFO2 + 4].try_into().unwrap());
    Ok(flags & dd::table::flags::ENCRYPTED != 0)
}

/// Puts the file of the table `name` of the AES dictionary `connection` is
/// to in the clear when `on` is false, if the Table object says it is
/// encrypted, and enciphers it when `on`, leaving the object as it is and the
/// index files as they are (a clear one stays so). Whether the table was
/// encrypted, `false` for any table of another dictionary.
pub(crate) fn table_file_encryption(
    session: &Session,
    connection: Handle,
    name: &str,
    on: bool,
) -> Result<bool, Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let aes = conn
        .with_database_opt(|db| db.scheme() != tflip_storage::crypto::Scheme::Rc4)
        .unwrap_or(false);
    let encrypted = {
        let dict = open_dict(&add_path)?;
        let table = find_named(&dict, Kind::Table, "table", name)?;
        let record = open_dictionary_file(&add_path)?
            .record(table.record())
            .map_err(|_| Error::CorruptTable)?;
        let flags = i32::from_le_bytes(record[INFO2..INFO2 + 4].try_into().unwrap());
        flags & dd::table::flags::ENCRYPTED != 0
    };
    if !encrypted {
        return Ok(false);
    }
    let rc4_key = super::rc4_tables::table_key(&conn);
    if !aes && rc4_key.is_none() {
        return Ok(false);
    }
    let settings = crate::table::TableSettings {
        options: crate::table::options::EXCLUSIVE,
        ..Default::default()
    };
    let indexes: Vec<_> = conn
        .with_database_opt(|db| db.table(name))
        .flatten()
        .map(|bound| bound.index_files)
        .unwrap_or_default()
        .into_iter()
        .filter(|index| !on && tflip_storage::adi::is_aes(index))
        .collect();
    let handle = crate::table::open(session, connection, name, settings)?;
    let changed = crate::table::get(session, handle).and_then(|table| match (on, aes) {
        (true, true) => table.encrypt_for_dictionary(&add_path, &[]),
        (false, true) => table.decrypt_for_dictionary(&indexes),
        (true, false) => table.encrypt_for_dictionary_rc4(&rc4_key.unwrap_or_default(), &[]),
        (false, false) => table.decrypt_for_dictionary_rc4(&indexes),
    });
    let _ = crate::table::close(session, handle);
    changed.map(|()| true)
}

/// The property ids each `AdsDDSet*Property` of the original knows, found by
/// trying every id up to 1800 and some above (`tools/ace/sql/ddpropscan.txt`);
/// any other is 5126 whatever the object's name and the user's rights are, and
/// 5042 for a user group.
pub(crate) mod known_properties {
    pub const TABLE: &[u16] = &[
        1, 3, 5, 200, 201, 202, 203, 209, 213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223,
    ];
    pub const FIELD: &[u16] = &[1, 3, 300, 301, 302, 303, 304, 311, 312, 313];
    /// The one id (the largest) that a table's and a field's setter answers
    /// with 5132, "no such named table", for a table that is there and
    /// whoever the user is.
    pub const NO_SUCH_TABLE: u16 = 65535;
    pub const INDEX: &[u16] = &[1];
    pub const VIEW: &[u16] = &[1, 5, 700, 704];
    pub const PROCEDURE: &[u16] = &[1, 800, 801, 802, 803, 805, 806];
    pub const USER: &[u16] = &[1, 3, 107, 113, 1101, 1104, 1105];
    pub const GROUP: &[u16] = &[1, 3];
    pub const PUBLICATION: &[u16] = &[1, 3, 1500];
    pub const ARTICLE: &[u16] = &[1, 1600, 1601, 1604, 1605, 1607, 1608, 1609];
    pub const SUBSCRIPTION: &[u16] = &[
        1, 3, 1700, 1701, 1702, 1703, 1704, 1705, 1706, 1707, 1709, 1710, 1711, 1712, 1713, 1715,
        1716,
    ];
    pub const DATABASE: &[u16] = &[
        1, 3, 5, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115,
        116, 117, 118, 120, 121, 122, 125, 127, 128, 129, 130, 131, 132,
    ];
}

/// The refusal of a property id the original does not know, with the object
/// the setter is for in the words it uses (`a table object`).
pub(crate) fn require_known_property(id: u16, known: &[u16], object: &str) -> Result<(), Error> {
    match known.contains(&id) {
        true => Ok(()),
        false => Err(Error::InvalidPropertyId.detailed(format!(
            "The specified property cannot be set for {object}."
        ))),
    }
}

/// Sets a property of a table in the dictionary (`AdsDDSetTableProperty`).
/// `ADS_DD_COMMENT` goes to the table's comment; the validation expression and
/// message, the memo block size, the trigger types and options, and the
/// user-defined property go to the table's property blob, verbatim as the caller
/// gives them, as the original engine stores them (a read-modify-write that
/// keeps the table's other properties). `ADS_DD_TABLE_PRIMARY_KEY` names an
/// index whose id the engine stores in the table's `Info1` (needed before a
/// referential-integrity rule can reference the table). Other properties — among
/// them the flags packed into the record header (permission level, caching,
/// concurrency) — are [`Error::NotImplemented`].
pub fn set_table_property_api(
    session: &Session,
    connection: Handle,
    table_name: &str,
    property_id: u16,
    value: &[u8],
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    if property_id == known_properties::NO_SUCH_TABLE {
        return Err(not_named("table"));
    }
    require_known_property(property_id, known_properties::TABLE, "a table object")?;
    let dict = open_dict(&add_path)?;
    let table = find_named_api(&dict, Kind::Table, "table", table_name)?;
    super::rights::require(&conn, 1, table_name, super::rights::ALTER, "")?;
    let slot = table.record();
    // ADS_DD_TABLE_PRIMARY_KEY: the value is the name of an index of the table
    // (empty to clear); the engine stores that index's id in the table's Info1
    // (0 for none). An unknown index name is refused with 5132. The index
    // record itself is untouched, as the engine leaves it (probed black-box).
    if property_id == 202 {
        let tag = value.split(|&b| b == 0).next().unwrap_or(value);
        let primary = match tag.is_empty() {
            true => 0,
            false => {
                let name = std::str::from_utf8(tag).map_err(|_| invalid_object_name(""))?;
                find_child(&dict, table.id, Kind::Index, "index", name)?.id as i32
            }
        };
        return set_object_info1(session, &conn, &add_path, slot, primary);
    }
    // ADS_DD_TABLE_ENCRYPTION: in a dictionary of AES tables the dictionary's
    // key is the table's; one of RC4 tables wants a table password.
    if property_id == 214 {
        drop(dict);
        return set_table_encryption(session, connection, table_name, flag_on(value), true)
            .map_err(ace_error::plain);
    }
    // The flag properties packed into the record's Info2: permission level,
    // caching and transaction-free. A read-modify-write that keeps the other
    // bits. The values the engine stores are a subset (web-delta and
    // concurrency, and caching "both", it silently drops), so the rest stay
    // [`Error::NotImplemented`].
    use dd::table::flags;
    let permission = (flags::PERMISSION_LEVEL_1 | flags::PERMISSION_LEVEL_3) as u32;
    let caching = 3u32 << flags::CACHING_SHIFT;
    let flag_bits = match property_id {
        216 => match value_u16(value) {
            // ADS_DD_TABLE_PERMISSION_LEVEL: 1, 2 (the default, no bit) or 3.
            1 => Some((permission, flags::PERMISSION_LEVEL_1 as u32)),
            2 => Some((permission, 0)),
            3 => Some((permission, flags::PERMISSION_LEVEL_3 as u32)),
            _ => return Err(Error::NotImplemented),
        },
        219 => match value_u16(value) {
            // ADS_DD_TABLE_CACHING: none, reads or writes (two bits at the top).
            mode @ 0..=2 => Some((caching, u32::from(mode) << flags::CACHING_SHIFT)),
            _ => return Err(Error::NotImplemented),
        },
        // ADS_DD_TABLE_TXN_FREE.
        220 => {
            let bit = flags::TRANSACTION_FREE as u32;
            Some((bit, if flag_on(value) { bit } else { 0 }))
        }
        _ => None,
    };
    if let Some((clear, set)) = flag_bits {
        return set_object_info2(session, &conn, &add_path, slot, clear, set);
    }
    set_property(
        session,
        &conn,
        &add_path,
        slot,
        Kind::Table,
        u32::from(property_id),
        value,
    )
}

/// Sets a property of a view in the dictionary (`AdsDDSetViewProperty`).
/// `ADS_DD_COMMENT` goes to the view's comment; `ADS_DD_VIEW_STMT` replaces its
/// SQL statement (and the stored length), as the original engine writes them.
/// Other properties are [`Error::NotImplemented`].
pub fn set_view_property_api(
    session: &Session,
    connection: Handle,
    view_name: &str,
    property_id: u16,
    value: &[u8],
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    require_known_property(property_id, known_properties::VIEW, "a view object")?;
    let dict = open_dict(&add_path)?;
    let view = find_named_api(&dict, Kind::View, "view", view_name)?;
    super::rights::require(&conn, 6, view_name, super::rights::ALTER, "")?;
    let slot = view.record();
    match property_id {
        ADS_DD_COMMENT => set_object_comment(session, &conn, &add_path, slot, comment_value(value)),
        // ADS_DD_VIEW_STMT: the statement text, verbatim, and its byte length
        // (the value's, NUL included) in the length slot, as the engine stores
        // them.
        700 => {
            let length = (value.len() as u16).to_le_bytes();
            let slots: &[(usize, &[u8])] = &[
                (dd::view::STATEMENT, value),
                (dd::view::STATEMENT_LENGTH, &length),
            ];
            set_object_slots(session, &conn, &add_path, slot, slots)
        }
        _ => Err(Error::NotImplemented),
    }
}

/// Sets property slot `index` of the object at record `slot` to `value`, a
/// read-modify-write of its property blob as the engine stores it (its other
/// properties are preserved). Then the index is rebuilt and the connection
/// reloaded.
pub(crate) fn set_object_slot(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    slot: u32,
    index: usize,
    value: &[u8],
) -> Result<(), Error> {
    set_object_slots(session, conn, add_path, slot, &[(index, value)])
}

/// Sets several property slots of the object at record `slot` in one
/// read-modify-write of its property blob (its other properties are preserved),
/// then rebuilds the index and reloads. Used where one property writes more than
/// one slot, as a view's statement writes both the text and its length.
pub(super) fn set_object_slots(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    slot: u32,
    slots: &[(usize, &[u8])],
) -> Result<(), Error> {
    let slots: Vec<_> = slots
        .iter()
        .map(|&(index, value)| (index, Some(value)))
        .collect();
    write_object_slots(session, conn, add_path, slot, &slots)
}

/// Edits the properties of the object at record `slot` of `file` (see
/// [`tflip_storage::dd::create::edit_properties`]) and writes back the
/// `Property` and `More Property` fields.
pub(super) fn edit_object_properties(
    file: &mut tflip_storage::dd::DictionaryFile,
    add_path: &Path,
    slot: u32,
    edit: impl FnOnce(&mut Vec<Option<Vec<u8>>>),
) -> Result<(), Error> {
    use tflip_storage::dd::{self, create};
    let mut record = file.record(slot).map_err(|_| Error::CorruptTable)?;
    create::edit_properties(&mut record, &dd::memo_path(add_path), edit)
        .map_err(|_| Error::CorruptTable)?;
    let (from, to) = (create::PROPERTY, create::MORE_PROPERTY + 8);
    file.write_part(slot, from, &record[from..to])
        .map_err(|_| Error::CorruptTable)
}

/// How a property of an object is stored in its record.
#[derive(Clone, Copy)]
pub(crate) enum Storage {
    /// In a property slot, as the value is given.
    Value(usize),
    /// By the presence of an empty slot; `inverted` for a flag the slot says
    /// is off by being there.
    Presence { slot: usize, inverted: bool },
}

/// Where the property `id` (`ADS_DD_*`) of an object of the kind `kind` is
/// stored, for the properties that are stored as they are given: the
/// comment, and what the `AdsDDSet*Property` calls and the system procedures
/// that change them both write.
pub(crate) fn storage(kind: tflip_storage::dd::Kind, id: u32) -> Option<Storage> {
    use Storage::{Presence, Value};
    use tflip_storage::dd::{self, Kind};
    let present = |slot| Presence {
        slot,
        inverted: false,
    };
    let absent = |slot| Presence {
        slot,
        inverted: true,
    };
    Some(match (kind, id) {
        (Kind::Table, 3) => Value(dd::table::USER_DEFINED),
        (Kind::Table, 200) => Value(dd::table::VALIDATION_EXPR),
        (Kind::Table, 201) => Value(dd::table::VALIDATION_MSG),
        (Kind::Table, 203) => absent(dd::table::NO_AUTO_CREATE),
        (Kind::Table, 213) => Value(dd::table::DEFAULT_INDEX),
        (Kind::Table, 215) => Value(dd::table::MEMO_BLOCK_SIZE),
        (Kind::Table, 217) => Value(dd::table::TRIGGER_TYPES),
        (Kind::Table, 218) => Value(dd::table::TRIGGER_OPTIONS),
        (Kind::Field, 3) => Value(dd::field::USER_DEFINED),
        (Kind::Field, 300) => Value(dd::field::DEFAULT_VALUE),
        (Kind::Field, 301) => absent(dd::field::NOT_NULL),
        (Kind::Field, 302) => Value(dd::field::MIN_VALUE),
        (Kind::Field, 303) => Value(dd::field::MAX_VALUE),
        (Kind::Field, 304) => Value(dd::field::VALIDATION_MSG),
        (Kind::User, 3) => Value(dd::user::USER_DEFINED),
        (Kind::User, 1105) => absent(dd::user::NO_OLD_PASSWORD),
        (Kind::Group, 3) => Value(dd::group::USER_DEFINED),
        (Kind::Procedure, 800) => Value(dd::procedure::INPUT),
        (Kind::Procedure, 801) => Value(dd::procedure::OUTPUT),
        (Kind::Procedure, 802) => Value(dd::procedure::DLL_NAME),
        (Kind::Procedure, 803) => Value(dd::procedure::DLL_FUNCTION_NAME),
        (Kind::Procedure, 804) => Value(dd::procedure::INVOKE_OPTION),
        (Kind::Procedure, 805 | 806) => Value(dd::procedure::SCRIPT),
        (Kind::Database, 3) => Value(dd::database::USER_DEFINED),
        (Kind::Database, 104) => absent(dd::database::RIGHTS_NOT_CHECKED),
        (Kind::Database, 107) => Value(dd::database::ENABLE_INTERNET),
        (Kind::Database, 108) => Value(dd::database::INTERNET_SECURITY_LEVEL),
        (Kind::Database, 109) => Value(dd::database::MAX_FAILED_ATTEMPTS),
        (Kind::Database, 111) => Value(dd::database::VERSION_MAJOR),
        (Kind::Database, 112) => Value(dd::database::VERSION_MINOR),
        (Kind::Database, 113) => Value(dd::database::LOGINS_DISABLED),
        (Kind::Database, 114) => Value(dd::database::LOGINS_DISABLED_MESSAGE),
        (Kind::Database, 115) => Value(dd::database::FTS_DELIMITERS),
        (Kind::Database, 116) => Value(dd::database::FTS_NOISE),
        (Kind::Database, 117) => Value(dd::database::FTS_DROP_CHARS),
        (Kind::Database, 118) => Value(dd::database::FTS_CONDITIONAL_CHARS),
        (Kind::Database, 121) => Value(dd::database::QUERY_LOG_TABLE),
        (Kind::Database, 122) => present(dd::database::ENCRYPT_COMMUNICATION),
        (Kind::Database, 125) => present(dd::database::DISABLE_DLL_CACHING),
        (Kind::Database, 131) => Value(dd::database::QUERY_VIA_ROOT),
        (Kind::Database, 132) => Value(dd::database::ENFORCE_MAX_FAILED_LOGINS),
        _ => return None,
    })
}

/// The slot the property `id` of an object of the kind `kind` is stored in as
/// given; one that is not stored so is a bug of the caller.
pub(crate) fn value_slot(kind: tflip_storage::dd::Kind, id: u32) -> usize {
    match storage(kind, id) {
        Some(Storage::Value(slot)) | Some(Storage::Presence { slot, .. }) => slot,
        None => unreachable!("property {id} of {kind:?} is not stored as given"),
    }
}

/// Sets the property `id` of the object at record `record`, of the kind
/// `kind`, to `value` as the `AdsDDSet*Property` calls do:
/// [`Error::NotImplemented`] for one that is not stored as it is given.
fn set_property(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    record: u32,
    kind: tflip_storage::dd::Kind,
    id: u32,
    value: &[u8],
) -> Result<(), Error> {
    if id == ADS_DD_COMMENT as u32 {
        return set_object_comment(session, conn, add_path, record, comment_value(value));
    }
    match storage(kind, id).ok_or(Error::NotImplemented)? {
        Storage::Value(slot) => set_object_slot(session, conn, add_path, record, slot, value),
        Storage::Presence { slot, inverted } => set_object_presence(
            session,
            conn,
            add_path,
            record,
            slot,
            flag_on(value) != inverted,
        ),
    }
}

/// Sets property slot `index` of the decoded `properties` to `value`, or unsets
/// it with `None`.
pub(super) fn set_slot(properties: &mut Vec<Option<Vec<u8>>>, index: usize, value: Option<&[u8]>) {
    if properties.len() <= index {
        properties.resize_with(index + 1, || None);
    }
    properties[index] = value.map(<[u8]>::to_vec);
}

/// Sets or clears a presence-flag property slot of the object at record `slot`
/// — a slot the engine stores by its presence, empty (such as a field's
/// `NOT_NULL` marker) — then rebuilds the index and reloads. The object's other
/// properties are preserved.
pub(crate) fn set_object_presence(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    slot: u32,
    index: usize,
    present: bool,
) -> Result<(), Error> {
    write_object_slots(
        session,
        conn,
        add_path,
        slot,
        &[(index, present.then_some(&[][..]))],
    )
}

/// Whether a property value (a `u16` flag the caller passes) is on.
pub(crate) fn flag_on(value: &[u8]) -> bool {
    value.iter().any(|&b| b != 0)
}

/// A property value read as the little-endian `u16` the caller passes (short
/// or empty buffers read as 0).
pub(super) fn value_u16(value: &[u8]) -> u16 {
    u16::from_le_bytes([
        value.first().copied().unwrap_or(0),
        value.get(1).copied().unwrap_or(0),
    ])
}

/// The little-endian `u32` a property value holds (zero-padded if shorter).
pub(super) fn value_u32(value: &[u8]) -> u32 {
    let mut bytes = [0u8; 4];
    let n = value.len().min(4);
    bytes[..n].copy_from_slice(&value[..n]);
    u32::from_le_bytes(bytes)
}

/// Sets the `Info1` of the object at record `slot` to `value`, then rebuilds and
/// reloads. A table's `Info1` is the id of its primary-key index (0 for none).
pub(crate) fn set_object_info1(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    slot: u32,
    value: i32,
) -> Result<(), Error> {
    {
        let mut file = open_dictionary_file(add_path)?;
        file.write_part(slot, INFO1, &value.to_le_bytes())
            .map_err(|_| Error::CorruptTable)?;
    }
    rebuild_dictionary(session, conn, add_path)
}

/// Clears the `clear` bits and sets the `set` bits of the `Info2` flags of the
/// object at record `slot` (a read-modify-write), then rebuilds and reloads.
pub(crate) fn set_object_info2(
    session: &Session,
    conn: &connection::Connection,
    add_path: &Path,
    slot: u32,
    clear: u32,
    set: u32,
) -> Result<(), Error> {
    {
        let mut file = open_dictionary_file(add_path)?;
        let record = file.record(slot).map_err(|_| Error::CorruptTable)?;
        let mut flags = u32::from_le_bytes(record[INFO2..INFO2 + 4].try_into().unwrap());
        flags = (flags & !clear) | set;
        file.write_part(slot, INFO2, &flags.to_le_bytes())
            .map_err(|_| Error::CorruptTable)?;
    }
    rebuild_dictionary(session, conn, add_path)
}

/// Sets a property of a field in the dictionary (`AdsDDSetFieldProperty`).
/// `ADS_DD_COMMENT` goes to the field's comment; the string-valued field
/// properties (default, minimum, maximum and validation message) go to the
/// field's property blob, verbatim as the caller gives them. Other properties
/// are [`Error::NotImplemented`].
pub fn set_field_property_api(
    session: &Session,
    connection: Handle,
    table_name: &str,
    field_name: &str,
    property_id: u16,
    value: &[u8],
) -> Result<(), Error> {
    use tflip_storage::dd::Kind;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    if property_id == known_properties::NO_SUCH_TABLE {
        return Err(not_named("table"));
    }
    require_known_property(property_id, known_properties::FIELD, "a field object")?;
    let dict = open_dict(&add_path)?;
    let table = find_named_api(&dict, Kind::Table, "table", table_name)?;
    super::rights::require(&conn, 1, table_name, super::rights::ALTER, "")?;
    let field = dict
        .child(table.id, Kind::Field, field_name)
        .ok_or_else(|| not_valid("The specified field is not part of the table."))?;
    let slot = field.record();
    // A default, minimum or maximum the field cannot take is refused, as the
    // original refuses it (`fixtures/dictionary/rules.json`).
    let property = match property_id {
        300 => Some(crate::constraints::Property::Default),
        302 | 303 => Some(crate::constraints::Property::Bound),
        _ => None,
    };
    if let Some(property) = property {
        let end = value.iter().position(|&b| b == 0).unwrap_or(value.len());
        let text = tflip_storage::cp1252::decode(&value[..end]);
        let settings = table::TableSettings::default();
        if let Ok(handle) = table::open(session, connection, table_name, settings) {
            let checked = table::get(session, handle)
                .and_then(|t| t.check_dictionary_value(field_name, property, &text));
            let _ = table::close(session, handle);
            checked?;
        }
    }
    set_property(
        session,
        &conn,
        &add_path,
        slot,
        Kind::Field,
        u32::from(property_id),
        value,
    )
}

/// Sets a property of an index order in the dictionary
/// (`AdsDDSetIndexProperty`). The index is named within its table, and its
/// comment is stored on the order (`Key`) record. Only `ADS_DD_COMMENT` is
/// handled; other properties are [`Error::NotImplemented`].
pub fn set_index_property_api(
    session: &Session,
    connection: Handle,
    table_name: &str,
    index_name: &str,
    property_id: u16,
    value: &[u8],
) -> Result<(), Error> {
    use tflip_storage::dd::Kind;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    require_known_property(property_id, known_properties::INDEX, "an Index object")?;
    let dict = open_dict(&add_path)?;
    let table = find_named_api(&dict, Kind::Table, "table", table_name)?;
    super::rights::require(&conn, 1, table_name, super::rights::ALTER, "")?;
    // The index ORDER is a `Key` object (Kind::Index) under the table.
    let order = find_child_api(&dict, table.id, Kind::Index, "index order", index_name)?;
    set_object_comment(
        session,
        &conn,
        &add_path,
        order.record(),
        comment_value(value),
    )
}

/// Sets a property of the dictionary's database object
/// (`AdsDDSetDatabaseProperty`). `ADS_DD_COMMENT` goes to the database's
/// comment; the value-slot properties — the customer version numbers, the
/// Internet and login settings, the full-text-search word lists, the query-log
/// table, the query-via-root option and the user-defined property — go to the
/// database's property blob, verbatim as the caller gives them, as the original
/// engine stores them (a read-modify-write that keeps the `ADSSYS` login secret
/// and the other properties). The presence-flag properties (verify access
/// rights, encrypt communication, disable DLL caching) toggle their empty slot.
/// The administrator password is written (`set_admin_password`); login-required
/// and the encryption settings are [`Error::NotImplemented`].
pub fn set_database_property_api(
    session: &Session,
    connection: Handle,
    property_id: u16,
    value: &[u8],
) -> Result<(), Error> {
    use tflip_storage::dd::{self};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    require_known_property(
        property_id,
        known_properties::DATABASE,
        "the database object",
    )?;
    // The administrator's password checks its length first, and who sets it
    // (`fixtures/sql/spddadminpw`).
    if property_id != ADS_DD_ADMIN_PASSWORD {
        super::rights::require_administrator(&conn)?;
    }
    if property_id == ADS_DD_ADMIN_PASSWORD {
        return super::users::set_admin_password(session, &conn, &add_path, value)
            .map_err(ace_error::plain);
    }
    if matches!(property_id, 106 | 120) {
        let slot = match property_id {
            106 => dd::database::ENCRYPT_NEW_TABLE,
            _ => dd::database::ENCRYPT_INDEXES,
        };
        return super::rc4_tables::set_flag(session, &conn, &add_path, slot, flag_on(value))
            .map_err(ace_error::plain);
    }
    let dict = open_dict(&add_path)?;
    let database = dict.database().ok_or(Error::NoDictionary)?;
    let slot = database.record();
    set_property(
        session,
        &conn,
        &add_path,
        slot,
        dd::Kind::Database,
        u32::from(property_id),
        value,
    )
}

/// Sets a property of a user in the dictionary (`AdsDDSetUserProperty`).
/// `ADS_DD_COMMENT` goes to the user's comment, `ADS_DD_USER_DEFINED_PROP` to its
/// user-defined slot, and `ADS_DD_REQUIRE_OLD_PASSWORD` to the presence of the
/// `NO_OLD_PASSWORD` slot (inverted: requiring the old password means no such
/// slot). Other properties — among them setting the password (1101) — are
/// [`Error::NotImplemented`].
pub fn set_user_property_api(
    session: &Session,
    connection: Handle,
    user_name: &str,
    property_id: u16,
    value: &[u8],
) -> Result<(), Error> {
    use tflip_storage::dd::Kind;
    // The rights properties are no ones to set, whoever the user is
    // (`fixtures/dictionary/freetables.json`).
    if (1001..=1005).contains(&property_id) {
        return Err(Error::InvalidPropertyId
            .detailed("The specified property cannot be set for a user object."));
    }
    let (conn, add_path) = dictionary_connection(session, connection)?;
    require_known_property(property_id, known_properties::USER, "a user object")?;
    // The password of a user is a string up to its NUL, cut to 20 bytes; the
    // name given with the current-password property is the old password.
    let text = |bytes: &[u8]| -> Vec<u8> {
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        bytes[..end.min(20)].to_vec()
    };
    if property_id == 1104 {
        return super::users::change_own_password(
            session,
            &conn,
            &add_path,
            user_name.as_bytes(),
            &text(value),
        )
        .map_err(ace_error::plain);
    }
    let administrator = user_name.eq_ignore_ascii_case("ADSSYS");
    let dict = open_dict(&add_path)?;
    // The administrator is no user object, and has a password to set.
    let user = match administrator && property_id == 1101 {
        true => None,
        false => Some(find_named_api(&dict, Kind::User, "user", user_name)?),
    };
    if property_id == 1101 {
        drop(dict);
        return super::users::set_user_password(session, &conn, &add_path, user_name, &text(value))
            .map_err(ace_error::plain);
    }
    super::rights::require(&conn, 8, user_name, super::rights::ALTER, "")?;
    let slot = user
        .map(|user| user.record())
        .ok_or(Error::NotImplemented)?;
    set_property(
        session,
        &conn,
        &add_path,
        slot,
        Kind::User,
        u32::from(property_id),
        value,
    )
}

/// Sets a property of a user group in the dictionary
/// (`AdsDDSetUserGroupProperty`). `ADS_DD_COMMENT` goes to the group's comment and
/// `ADS_DD_USER_DEFINED_PROP` to its user-defined slot. Others are
/// [`Error::NotImplemented`].
pub fn set_user_group_property_api(
    session: &Session,
    connection: Handle,
    group_name: &str,
    property_id: u16,
    value: &[u8],
) -> Result<(), Error> {
    use tflip_storage::dd::Kind;
    if (1001..=1005).contains(&property_id) {
        return Err(Error::NotImplemented.detailed("The specified property cannot be set."));
    }
    let (conn, add_path) = dictionary_connection(session, connection)?;
    if !known_properties::GROUP.contains(&property_id) {
        return Err(Error::NotImplemented.detailed("The specified property cannot be set."));
    }
    let dict = open_dict(&add_path)?;
    let group = find_named_api(&dict, Kind::Group, "user group", group_name)?;
    super::rights::require(&conn, 9, group_name, super::rights::ALTER, "")?;
    let slot = group.record();
    set_property(
        session,
        &conn,
        &add_path,
        slot,
        Kind::Group,
        u32::from(property_id),
        value,
    )
}

/// Sets a property of a stored procedure in the dictionary
/// (`AdsDDSetProcedureProperty`). `ADS_DD_COMMENT` goes to its comment; the
/// input/output parameter lists, the DLL name and function, the invoke option and
/// the script (narrow and wide) go to the procedure's property slots, as the
/// original engine stores them. A procedure has no user-defined property (the
/// engine refuses it); other properties are [`Error::NotImplemented`].
pub fn set_procedure_property_api(
    session: &Session,
    connection: Handle,
    procedure_name: &str,
    property_id: u16,
    value: &[u8],
) -> Result<(), Error> {
    use tflip_storage::dd::Kind;
    let (conn, add_path) = dictionary_connection(session, connection)?;
    require_known_property(
        property_id,
        known_properties::PROCEDURE,
        "a procedure object",
    )?;
    let dict = open_dict(&add_path)?;
    let procedure = find_named_api(&dict, Kind::Procedure, "stored procedure", procedure_name)?;
    super::rights::require(&conn, 10, procedure_name, super::rights::ALTER, "")?;
    let slot = procedure.record();
    set_property(
        session,
        &conn,
        &add_path,
        slot,
        Kind::Procedure,
        u32::from(property_id),
        value,
    )
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    /// Replays the steps of `tools/ace/dd/proplong.c` in a new dictionary with
    /// the table `t` added, and compares what is left with the engine's
    /// (`fixtures/dictionary/proplong/<scenario>`) byte for byte, the
    /// login secrets aside.
    fn check_proplong(scenario: &str, steps: &dyn Fn(&Session, Handle)) {
        check_long("proplong", scenario, steps);
    }

    /// [`check_proplong`] for a scenario in `fixtures/dictionary/<folder>`.
    fn check_long(folder: &str, scenario: &str, steps: &dyn Fn(&Session, Handle)) {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/dictionary")
            .join(folder)
            .join(scenario);
        let session = Session::new();
        let dir = tflip_storage::testing::temp_root().join(format!(
            "tflip-ddproplong-{scenario}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        copy_free(&fixtures.join("t.adt"), &dir.join("t.adt"));
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        add_table_api(&session, connect, "t", "t.adt", TableType::Adt, 1, "", "").unwrap();
        steps(&session, connect);

        let ours = mask_user_secrets(std::fs::read(&add).unwrap());
        let golden = mask_user_secrets(std::fs::read(fixtures.join("db.add")).unwrap());
        assert_eq!(ours.len(), golden.len(), "{scenario}: add length");
        let data = u32::from_le_bytes(ours[0x20..0x24].try_into().unwrap()) as usize;
        let rl = 524;
        for record in 1..(ours.len() - data) / rl {
            let at = data + record * rl;
            let differing = |a: &[u8], b: &[u8]| -> Vec<usize> {
                (0..a.len().max(b.len()))
                    .filter(|&i| a.get(i) != b.get(i))
                    .collect()
            };
            assert_eq!(
                differing(&ours[at..at + rl], &golden[at..at + rl]),
                Vec::<usize>::new(),
                "{scenario}: record {}",
                record + 1
            );
        }
        for file in ["db.am", "db.ai"] {
            let golden = std::fs::read(fixtures.join(file)).unwrap();
            let ours = std::fs::read(dir.join(file)).unwrap();
            let first = (0..ours.len().max(golden.len())).find(|&i| ours.get(i) != golden.get(i));
            assert_eq!(first, None, "{scenario}: {file}, first differing byte");
        }
        connection::disconnect(&session, connect).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A statement `length` bytes long, as `tools/ace/dd/proplong.c` makes it.
    fn long_sql(length: usize) -> String {
        let head = "SELECT id FROM t WHERE s <> '";
        match length.checked_sub(head.len() + 1) {
            Some(fill) => format!("{head}{}'", "x".repeat(fill)),
            None => "SELECT id FROM t".to_owned(),
        }
    }

    /// A view's statement and comment set after it was added, growing past the
    /// record's property field, replaced by a longer or shorter one, and
    /// shrinking back into it, are stored as the original engine stores them
    /// (`fixtures/dictionary/proplong/viewlater`), and read back whole.
    #[test]
    fn sets_long_view_properties_like_the_engine() {
        check_proplong("viewlater", &|session, connect| {
            let stmt = |name: &str, length: usize| {
                let sql = format!("{}\0", long_sql(length));
                set_view_property_api(session, connect, name, 700, sql.as_bytes()).unwrap();
            };
            let comment = |name: &str, text: &str| {
                let text = format!("{text}\0");
                set_view_property_api(session, connect, name, 1, text.as_bytes()).unwrap();
            };
            let add = |name: &str, length: usize| {
                add_view_api(session, connect, name, &long_sql(length), "").unwrap();
            };
            add("v1", 100);
            stmt("v1", 300);
            add("v2", 100);
            stmt("v2", 1000);
            comment("v2", "note");
            add("v3", 300);
            comment("v3", "c3");
            stmt("v3", 1000);
            stmt("v3", 300);
            comment("v3", "c3 longer comment");
            add("v4", 1000);
            stmt("v4", 300);
            stmt("v4", 100);
            stmt("v4", 300);
            add("v5", 3000);
            comment("v5", "five");
            stmt("v5", 50);
            add("v6", 100);
            stmt("v6", 268);
            stmt("v6", 269);
            stmt("v6", 268);
            add("v7", 300);
            stmt("v7", 301);
            stmt("v7", 5000);
            stmt("v7", 3000);

            let conn = connection::get(session, connect).unwrap();
            let read = |name: &str| conn.with_database_opt(|db| db.view(name)).flatten();
            assert_eq!(read("v7").unwrap().len(), 3000);
            assert_eq!(read("v3").unwrap(), long_sql(300));
            assert_eq!(read("v5").unwrap(), long_sql(50));
        });
    }

    /// The other setters that can overflow the record's property field: a
    /// procedure's script, a table's, a field's and a user's user-defined
    /// property, set long and short again
    /// (`fixtures/dictionary/proplong/proplater`).
    #[test]
    fn sets_long_object_properties_like_the_engine() {
        check_proplong("proplater", &|session, connect| {
            add_procedure_api(
                session,
                connect,
                "p1",
                "SELECT 1 FROM t",
                "",
                0x4,
                "a,I;",
                "b,I;",
                "",
            )
            .unwrap();
            create_user_api(session, connect, "", "alice", "secret", "").unwrap();
            let quoted = |n: usize| format!("'{}'\0", "x".repeat(n - 2));
            let script = format!("{}\0", long_sql(400));
            set_procedure_property_api(session, connect, "p1", 805, script.as_bytes()).unwrap();
            set_procedure_property_api(session, connect, "p1", 1, b"pc\0").unwrap();
            let script = format!("{}\0", long_sql(60));
            set_procedure_property_api(session, connect, "p1", 805, script.as_bytes()).unwrap();
            set_table_property_api(session, connect, "t", 3, quoted(500).as_bytes()).unwrap();
            set_table_property_api(session, connect, "t", 3, b"ud\0").unwrap();
            set_field_property_api(session, connect, "t", "s", 3, quoted(400).as_bytes()).unwrap();
            set_user_property_api(session, connect, "alice", 3, quoted(400).as_bytes()).unwrap();
            set_user_property_api(session, connect, "alice", 1, b"hello\0").unwrap();
        });
    }

    /// A parameter list `n` bytes long, as `tools/ace/dd/proclong.c` makes it.
    fn param_list(n: usize) -> String {
        let mut list = String::new();
        let mut k = 0;
        while n - list.len() >= 14 {
            list += &format!("p{k:03},I;");
            k += 1;
        }
        let width = 3 + (n - list.len() - 7);
        list + &format!("p{k:0width$},I;")
    }

    /// Procedures with parameter lists longer than the record's property
    /// field, added, and set long and short again, are stored as the original
    /// engine stores them (`fixtures/dictionary/proclong/proclongapi`).
    #[test]
    fn adds_procedures_with_long_parameters_like_the_engine() {
        check_long("proclong", "proclongapi", &|session, connect| {
            let add =
                |name: &str, container: &str, option: u32, input: &str, output: &str, c: &str| {
                    let function = if option == 4 { "" } else { "Func" };
                    add_procedure_api(
                        session, connect, name, container, function, option, input, output, c,
                    )
                    .unwrap();
                };
            const LENGTHS: [usize; 14] = [
                100, 240, 250, 255, 258, 259, 260, 261, 262, 265, 270, 275, 276, 300,
            ];
            let script = "SELECT 1 FROM t";
            for n in LENGTHS {
                add(&format!("pi{n}"), script, 4, &param_list(n), "", "");
            }
            for n in LENGTHS {
                add(&format!("po{n}"), script, 4, "a,I;", &param_list(n), "");
            }
            add("pio", script, 4, &param_list(150), &param_list(150), "");
            add(
                "pic",
                script,
                4,
                &param_list(250),
                &param_list(20),
                "a comment",
            );
            let long = format!("SELECT id FROM t WHERE s <> '{}'", "x".repeat(56));
            add("pis", &long, 4, &param_list(150), "b,I;", "");
            add(
                "pex",
                ".\\x.aep",
                1,
                &param_list(150),
                &param_list(150),
                "ext comment",
            );
            add("pex2", ".\\x.aep", 1, &param_list(100), "", "");
            let set = |name: &str, id: u16, n: usize| {
                let value = format!("{}\0", param_list(n));
                set_procedure_property_api(session, connect, name, id, value.as_bytes()).unwrap();
            };
            set("pi100", 800, 400);
            set("pi100", 800, 20);
            set("po100", 801, 300);
            set("po240", 800, 100);
        });
    }

    /// Parameter lists whose continuation fills more than a slab's chunk (32
    /// blocks) go in extents (`fixtures/dictionary/proclong/proclongext`).
    #[test]
    fn adds_procedures_with_huge_parameters_like_the_engine() {
        check_long("proclong", "proclongext", &|session, connect| {
            let script = "SELECT 1 FROM t";
            let add = |name: &str, input: &str, output: &str| {
                add_procedure_api(session, connect, name, script, "", 4, input, output, "")
                    .unwrap();
            };
            add("pe1", &param_list(600), "");
            add("pe2", "a,I;", &param_list(1500));
            add("pe3", &param_list(300), "");
        });
    }

    /// A view, a procedure, a user, a group and a table with a field, each
    /// with properties continued in `More Property`, removed: the original
    /// engine frees that memo block, and then the comment's
    /// (`fixtures/dictionary/removelong/removed`).
    #[test]
    fn removes_long_objects_like_the_engine() {
        check_long("removelong", "removed", &|session, connect| {
            let quoted = |n: usize| format!("'{}'\0", "x".repeat(n - 2));
            add_view_api(session, connect, "v1", &long_sql(400), "").unwrap();
            set_view_property_api(session, connect, "v1", 1, b"vc\0").unwrap();
            add_procedure_api(
                session,
                connect,
                "p1",
                "SELECT 1 FROM t",
                "",
                4,
                &param_list(259),
                "",
                "pc",
            )
            .unwrap();
            create_user_api(session, connect, "", "alice", "secret", "").unwrap();
            set_user_property_api(session, connect, "alice", 3, quoted(400).as_bytes()).unwrap();
            set_user_property_api(session, connect, "alice", 1, b"uc\0").unwrap();
            create_group_api(session, connect, "grp", "gd").unwrap();
            set_user_group_property_api(session, connect, "grp", 3, quoted(300).as_bytes())
                .unwrap();
            set_table_property_api(session, connect, "t", 3, quoted(400).as_bytes()).unwrap();
            set_field_property_api(session, connect, "t", "s", 3, quoted(300).as_bytes()).unwrap();

            remove_view_api(session, connect, "v1").unwrap();
            delete_user_api(session, connect, "alice").unwrap();
            remove_procedure_api(session, connect, "p1").unwrap();
            delete_group_api(session, connect, "grp").unwrap();
            remove_table_api(session, connect, "t", false).unwrap();
        });
    }

    /// Views with statements long enough for extents, removed and added
    /// again, take their runs from the free ones as the original engine does
    /// (`fixtures/dictionary/removelong/extents`).
    #[test]
    fn takes_extents_from_free_runs_like_the_engine() {
        check_long("removelong", "extents", &|session, connect| {
            let add = |name: &str, length: usize| {
                add_view_api(session, connect, name, &long_sql(length), "").unwrap();
            };
            let remove = |name: &str| remove_view_api(session, connect, name).unwrap();
            add("v1", 3000);
            add("v2", 2000);
            add("v3", 4000);
            add("v4", 500);
            remove("v2");
            add("v5", 1000);
            add("v6", 1500);
            remove("v1");
            remove("v3");
            add("v7", 6000);
            add("v8", 300);
            remove("v4");
            remove("v5");
            add("v9", 2500);
            add("v10", 700);
            add("v11", 900);
        });
    }

    #[test]
    fn sets_object_comments_like_the_engine() {
        let session = Session::new();
        let fx = ops_fixtures();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddsetprop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");

        // Dict + table t (no index) + view v, then a comment on each (the
        // engine's order: table first, then view).
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        copy_free(&fx.join("t.adt"), &dir.join("t.adt"));
        add_table_api(&session, connect, "t", "t.adt", TableType::Adt, 1, "", "").unwrap();
        add_view_api(&session, connect, "v", "SELECT * FROM t", "").unwrap();
        set_table_property_api(&session, connect, "t", 1, b"table note\0").unwrap();
        set_view_property_api(&session, connect, "v", 1, b"view note\0").unwrap();

        let ours = std::fs::read(&add).unwrap();
        assert_eq!(
            mask_login(ours),
            mask_login(std::fs::read(fx.join("setprop.add")).unwrap()),
            ".add"
        );
        assert_eq!(
            std::fs::read(dir.join("db.am")).unwrap(),
            std::fs::read(fx.join("setprop.am")).unwrap(),
            ".am"
        );
        assert_eq!(
            std::fs::read(dir.join("db.ai")).unwrap(),
            std::fs::read(fx.join("setprop.ai")).unwrap(),
            ".ai"
        );
        // A property tflip does not handle is reported, not written.
        assert!(matches!(
            set_table_property_api(&session, connect, "t", 205, b"x\0"),
            Err(Error::Detailed(..))
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sets_user_group_and_procedure_properties_like_the_engine() {
        let session = Session::new();
        let fx = ops_fixtures();
        let run = |label: &str, build: &dyn Fn(&Session, Handle)| {
            let dir = tflip_storage::testing::temp_root()
                .join(format!("tflip-ddprop-{label}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let add = dir.join(format!("{label}.add"));
            let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
            build(&session, connect);
            for ext in ["add", "am", "ai"] {
                let ours = std::fs::read(dir.join(format!("{label}.{ext}"))).unwrap();
                let want = std::fs::read(fx.join(format!("{label}.{ext}"))).unwrap();
                let (ours, want) = if ext == "add" {
                    (mask_user_secrets(ours), mask_user_secrets(want))
                } else {
                    (ours, want)
                };
                assert_eq!(ours, want, "{label}: .{ext}");
            }
            connection::disconnect(&session, connect).ok();
            let _ = std::fs::remove_dir_all(&dir);
        };

        // A user: comment, user-defined property, require-old-password off.
        run("userprop", &|session, connect| {
            create_user_api(session, connect, "", "alice", "secret", "").unwrap();
            set_user_property_api(session, connect, "alice", 1, b"user note\0").unwrap();
            set_user_property_api(session, connect, "alice", 3, b"ud\0").unwrap();
            set_user_property_api(session, connect, "alice", 1105, &0u16.to_le_bytes()).unwrap();
        });

        // A group: comment and user-defined property.
        run("groupprop", &|session, connect| {
            create_group_api(session, connect, "g1", "").unwrap();
            set_user_group_property_api(session, connect, "g1", 1, b"group note\0").unwrap();
            set_user_group_property_api(session, connect, "g1", 3, b"gd\0").unwrap();
        });

        // A SQL-script procedure: comment and a replaced script.
        run("procprop", &|session, connect| {
            add_procedure_api(
                session,
                connect,
                "p",
                " INSERT INTO __output VALUES (1); ",
                "",
                0x4,
                "a,Integer;",
                "",
                "",
            )
            .unwrap();
            set_procedure_property_api(session, connect, "p", 1, b"proc note\0").unwrap();
            set_procedure_property_api(
                session,
                connect,
                "p",
                805,
                b" INSERT INTO __output VALUES (2); \0",
            )
            .unwrap();
        });

        // A procedure has no user-defined property, as the engine refuses it.
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddprop-x-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("x.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        add_procedure_api(
            &session,
            connect,
            "p",
            " INSERT INTO __output VALUES (1); ",
            "",
            0x4,
            "a,Integer;",
            "",
            "",
        )
        .unwrap();
        assert!(matches!(
            set_procedure_property_api(&session, connect, "p", 3, b"x\0"),
            Err(Error::Detailed(..))
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sets_field_and_database_properties_like_the_engine() {
        let session = Session::new();
        let fx = ops_fixtures();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddfieldprop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");

        // Dict + table t (fields id, s), then the field's comment and its
        // string-valued properties, then the database comment and version, in
        // the engine's order.
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        copy_free(&fx.join("t.adt"), &dir.join("t.adt"));
        add_table_api(&session, connect, "t", "t.adt", TableType::Adt, 1, "", "").unwrap();
        set_field_property_api(&session, connect, "t", "s", 1, b"field note\0").unwrap();
        set_field_property_api(&session, connect, "t", "s", 300, b"xx\0").unwrap();
        set_field_property_api(&session, connect, "t", "s", 302, b"aa\0").unwrap();
        set_field_property_api(&session, connect, "t", "s", 303, b"zz\0").unwrap();
        set_field_property_api(&session, connect, "t", "s", 304, b"bad\0").unwrap();
        set_database_property_api(&session, connect, 1, b"db note\0").unwrap();
        set_database_property_api(&session, connect, 111, &7u16.to_le_bytes()).unwrap();

        assert_eq!(
            mask_login(std::fs::read(&add).unwrap()),
            mask_login(std::fs::read(fx.join("fieldprop.add")).unwrap()),
            ".add"
        );
        assert_eq!(
            std::fs::read(dir.join("db.am")).unwrap(),
            std::fs::read(fx.join("fieldprop.am")).unwrap(),
            ".am"
        );
        assert_eq!(
            std::fs::read(dir.join("db.ai")).unwrap(),
            std::fs::read(fx.join("fieldprop.ai")).unwrap(),
            ".ai"
        );

        // A field property tflip does not handle, and an unknown field, are
        // reported, not written.
        assert!(matches!(
            set_field_property_api(&session, connect, "t", "s", 306, b"\0"),
            Err(Error::Detailed(..))
        ));
        assert!(matches!(
            set_field_property_api(&session, connect, "t", "nope", 300, b"x\0"),
            Err(Error::Sql(_))
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sets_table_and_field_blob_properties_like_the_engine() {
        let session = Session::new();
        let fx = ops_fixtures();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddtblprop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");

        // Dict + table t, then the table's property-blob properties and a field's
        // user-defined property, in the engine's order.
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        copy_free(&fx.join("t.adt"), &dir.join("t.adt"));
        add_table_api(&session, connect, "t", "t.adt", TableType::Adt, 1, "", "").unwrap();
        set_table_property_api(&session, connect, "t", 200, b"id > 0\0").unwrap();
        set_table_property_api(&session, connect, "t", 201, b"bad row\0").unwrap();
        set_table_property_api(&session, connect, "t", 215, &1024u16.to_le_bytes()).unwrap();
        set_table_property_api(&session, connect, "t", 217, &0x0au32.to_le_bytes()).unwrap();
        set_table_property_api(&session, connect, "t", 218, &0x05u32.to_le_bytes()).unwrap();
        set_table_property_api(&session, connect, "t", 3, b"table udef\0").unwrap();
        set_field_property_api(&session, connect, "t", "s", 3, b"field udef\0").unwrap();

        assert_eq!(
            mask_login(std::fs::read(&add).unwrap()),
            mask_login(std::fs::read(fx.join("tblprop.add")).unwrap()),
            ".add"
        );
        assert_eq!(
            std::fs::read(dir.join("db.am")).unwrap(),
            std::fs::read(fx.join("tblprop.am")).unwrap(),
            ".am"
        );
        assert_eq!(
            std::fs::read(dir.join("db.ai")).unwrap(),
            std::fs::read(fx.join("tblprop.ai")).unwrap(),
            ".ai"
        );

        // A table property tflip does not handle (the table's path is derived,
        // not set) is reported, not written.
        assert!(matches!(
            set_table_property_api(&session, connect, "t", 205, b"x\0"),
            Err(Error::Detailed(..))
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sets_default_index_and_view_statement_like_the_engine() {
        let session = Session::new();
        let fx = ops_fixtures();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddopsset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // The ops base (dict + table t + index file t.adi + view v), then the
        // table's default index and the view's statement.
        let connect = build_ops_base(&session, &dir);
        set_table_property_api(&session, connect, "t", 213, b"BY_ID\0").unwrap();
        set_view_property_api(&session, connect, "v", 700, b"SELECT id FROM t\0").unwrap();

        assert_eq!(
            mask_login(std::fs::read(dir.join("db.add")).unwrap()),
            mask_login(std::fs::read(fx.join("opsset.add")).unwrap()),
            ".add"
        );
        assert_eq!(
            std::fs::read(dir.join("db.ai")).unwrap(),
            std::fs::read(fx.join("opsset.ai")).unwrap(),
            ".ai"
        );
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sets_table_header_flags_like_the_engine() {
        let session = Session::new();
        let fx = ops_fixtures();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddflag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");

        // Dict + table t, then its Info2 flag properties: permission level 3,
        // caching reads, transaction-free.
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        copy_free(&fx.join("t.adt"), &dir.join("t.adt"));
        add_table_api(&session, connect, "t", "t.adt", TableType::Adt, 1, "", "").unwrap();
        set_table_property_api(&session, connect, "t", 216, &3u16.to_le_bytes()).unwrap();
        set_table_property_api(&session, connect, "t", 219, &1u16.to_le_bytes()).unwrap();
        set_table_property_api(&session, connect, "t", 220, &1u16.to_le_bytes()).unwrap();

        assert_eq!(
            mask_login(std::fs::read(&add).unwrap()),
            mask_login(std::fs::read(fx.join("flagprop.add")).unwrap()),
            ".add"
        );
        assert_eq!(
            std::fs::read(dir.join("db.ai")).unwrap(),
            std::fs::read(fx.join("flagprop.ai")).unwrap(),
            ".ai"
        );

        // Caching "both" and web-delta are among the flags the engine drops;
        // tflip reports them rather than guessing.
        assert_eq!(
            set_table_property_api(&session, connect, "t", 219, &3u16.to_le_bytes()),
            Err(Error::NotImplemented)
        );
        assert_eq!(
            set_table_property_api(&session, connect, "t", 222, &1u16.to_le_bytes()),
            Err(Error::NotImplemented)
        );
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sets_presence_flag_properties_like_the_engine() {
        let session = Session::new();
        let fx = ops_fixtures();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddpres-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");

        // Dict + table t, then the presence-flag properties in the engine's
        // order: database verify-rights (on, clears RIGHTS_NOT_CHECKED),
        // encrypt-communication and disable-DLL-caching (on, set their empty
        // slot), the table's auto-create (on, clears NO_AUTO_CREATE), and the
        // field's can-null (off, sets the empty NOT_NULL slot).
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        copy_free(&fx.join("t.adt"), &dir.join("t.adt"));
        add_table_api(&session, connect, "t", "t.adt", TableType::Adt, 1, "", "").unwrap();
        set_database_property_api(&session, connect, 104, &1u16.to_le_bytes()).unwrap();
        set_database_property_api(&session, connect, 122, &1u16.to_le_bytes()).unwrap();
        set_database_property_api(&session, connect, 125, &1u16.to_le_bytes()).unwrap();
        set_table_property_api(&session, connect, "t", 203, &1u16.to_le_bytes()).unwrap();
        set_field_property_api(&session, connect, "t", "s", 301, &0u16.to_le_bytes()).unwrap();

        assert_eq!(
            mask_db_secret(std::fs::read(&add).unwrap()),
            mask_db_secret(std::fs::read(fx.join("presprop.add")).unwrap()),
            ".add"
        );
        assert_eq!(
            std::fs::read(dir.join("db.ai")).unwrap(),
            std::fs::read(fx.join("presprop.ai")).unwrap(),
            ".ai"
        );
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sets_database_value_properties_like_the_engine() {
        let session = Session::new();
        let fx = ops_fixtures();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-dddbprop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");

        // A fresh dictionary, then its value-slot database properties in the
        // engine's order. No tables: the Database object is record 0.
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        let set = |id, v: &[u8]| set_database_property_api(&session, connect, id, v).unwrap();
        set(3, b"db udef\0");
        set(107, &1u16.to_le_bytes());
        set(108, &1u16.to_le_bytes());
        set(109, &7u16.to_le_bytes());
        set(113, &1u16.to_le_bytes());
        set(114, b"no logins\0");
        set(115, b".,;\0");
        set(116, b"the a an\0");
        set(117, b"-'\0");
        set(118, b"&\0");
        set(121, b"qlog\0");
        set(131, &2u32.to_le_bytes());
        set(132, &1u16.to_le_bytes());

        assert_eq!(
            mask_db_secret(std::fs::read(&add).unwrap()),
            mask_db_secret(std::fs::read(fx.join("dbprop.add")).unwrap()),
            ".add"
        );
        assert_eq!(
            std::fs::read(dir.join("db.ai")).unwrap(),
            std::fs::read(fx.join("dbprop.ai")).unwrap(),
            ".ai"
        );

        // A property stored as a presence flag is not handled yet.
        assert_eq!(
            set_database_property_api(&session, connect, 103, &1u16.to_le_bytes()),
            Err(Error::NotImplemented)
        );
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sets_an_index_comment_like_the_engine() {
        let session = Session::new();
        let fx = ops_fixtures();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddidxprop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");

        // Dict + table t + index file ix.adi (order BY_ID), then a comment on
        // the index order.
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        copy_free(&fx.join("t.adt"), &dir.join("t.adt"));
        std::fs::copy(fx.join("ix.adi"), dir.join("ix.adi")).unwrap();
        add_table_api(&session, connect, "t", "t.adt", TableType::Adt, 1, "", "").unwrap();
        add_index_file_api(&session, connect, "t", "ix.adi", "").unwrap();
        set_index_property_api(&session, connect, "t", "BY_ID", 1, b"index note\0").unwrap();

        assert_eq!(
            mask_login(std::fs::read(&add).unwrap()),
            mask_login(std::fs::read(fx.join("idxprop.add")).unwrap()),
            ".add"
        );
        assert_eq!(
            std::fs::read(dir.join("db.am")).unwrap(),
            std::fs::read(fx.join("idxprop.am")).unwrap(),
            ".am"
        );
        assert_eq!(
            std::fs::read(dir.join("db.ai")).unwrap(),
            std::fs::read(fx.join("idxprop.ai")).unwrap(),
            ".ai"
        );

        // A property the engine does not know of an index is refused, an
        // unknown index name is an error.
        assert!(matches!(
            set_index_property_api(&session, connect, "t", "BY_ID", 403, b"\0\0\0\0"),
            Err(Error::Detailed(..))
        ));
        assert!(matches!(
            set_index_property_api(&session, connect, "t", "nope", 1, b"x\0"),
            Err(Error::Sql(_))
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
