//! `sp_SetDDEncryptionType`: taking a dictionary to another encryption type
//! (`fixtures/aes/convert`, made by `tools/ace/crypto/aesconvert.sh`).
//!
//! What the original does, found by converting one dictionary of every layout
//! to every type:
//!
//! - The dictionary key `K` and the ADSSYS password stay. Every secret is
//!   enciphered again under the new cipher with the same `K`, but a user's
//!   password cannot be (it is hashed into the secret), so when the cipher
//!   changes (RC4, AES-128 and AES-256 are three) every user's password
//!   becomes empty. The Database object's login slot 0 is drawn anew every
//!   time.
//! - A dictionary of the default layout that stays in it (RC4 to RC4, plain
//!   to RC4) is changed in place: its flag, and with it its records, memo
//!   values and index, enciphered or not.
//! - Any other conversion writes a dictionary anew, the records of the
//!   objects that are left packed into the slots from the first, and leaves
//!   the dictionary it started from, with the new secrets and the new type
//!   in it (and in the clear), as `<name>.add.BAK` and `<name>.am.BAK`. An
//!   AES dictionary is written anew to keep its records in the clear, and
//!   when it comes from the default layout or is to have it.

use std::path::Path;

use tflip_storage::crypto::{self, Scheme};
use tflip_storage::dd::{self, Dictionary, Kind, Object, create::NewObject};

use super::*;

/// The type a dictionary is converted to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Rc4,
    /// AES-128 or AES-256, by the type its tables take (5 or 6).
    Aes(u8),
}

impl Target {
    fn scheme(self) -> Scheme {
        match self {
            Target::Rc4 => Scheme::Rc4,
            Target::Aes(tflip_storage::adt::ENCRYPTION_AES128) => Scheme::Aes128,
            Target::Aes(_) => Scheme::Aes256,
        }
    }

    /// The version of the dictionary the Database object keeps, the engine
    /// that reads it and the type of the tables: AES tables need version 5,
    /// RC4 ones 4 whatever the dictionary holds (the original does not look).
    fn database(self) -> (u32, &'static [u8], u32) {
        match self {
            Target::Aes(kind) => (5, b"10.1\0", u32::from(kind)),
            Target::Rc4 => (4, b"8.1\0", u32::from(tflip_storage::adt::ENCRYPTION_RC4)),
        }
    }
}

fn corrupt<E>(_: E) -> Error {
    Error::CorruptTable
}

/// The secrets of a dictionary, enciphered again for another cipher with the
/// same dictionary key.
struct Secrets<'a> {
    old: Scheme,
    new: Scheme,
    key: &'a [u8],
    admin: &'a [u8],
    /// The version, the engine it needs and the type the Database object
    /// takes when it is written again.
    database: Option<(u32, &'static [u8], u32)>,
}

impl Secrets<'_> {
    fn changes(&self) -> bool {
        self.old != self.new
    }

    /// `data`, which `K` enciphers at `item`, enciphered under the new cipher.
    fn recrypt(&self, data: &mut [u8], item: u32) {
        if self.changes() {
            crypto::crypt(self.old, self.key, item, data);
            crypto::crypt(self.new, self.key, item, data);
        }
    }

    /// The slots of `object`, which was the record `old` and is to be the
    /// record `record`, as the new cipher keeps them.
    fn slots(&self, object: &Object, record: u32) -> Vec<(usize, Vec<u8>)> {
        let id = object.id;
        let mut all: Vec<(usize, Vec<u8>)> = object
            .slots()
            .map(|(slot, bytes)| {
                let mut data = bytes.to_vec();
                match (object.kind, slot) {
                    (Kind::Database, 0) => data = login_salts().slot0.to_vec(),
                    (Kind::Database, dd::database::LOGIN_NOT_REQUIRED) if self.changes() => {
                        data = login_salts().slot4.to_vec();
                    }
                    (Kind::Database, 1) => data = self.admin_slot(object, bytes),
                    (Kind::User, dd::user::PASSWORD_SECRET) => {
                        data = self.password_secret(object, bytes, record);
                    }
                    (Kind::User, dd::user::GROUP_MEMBERSHIP) => {
                        if self.changes() {
                            crypto::group_membership(self.old, self.key, &mut data);
                            crypto::group_membership(self.new, self.key, &mut data);
                        }
                    }
                    (Kind::User, dd::user::BUILT_IN_MEMBERSHIP) => {
                        self.recrypt(&mut data, crypto::built_in_membership_item(id));
                    }
                    (Kind::User | Kind::Group, dd::permission::CREATE_RIGHTS) => {
                        self.recrypt(&mut data, crypto::permission_item(id, slot as u32));
                    }
                    (
                        Kind::Permission,
                        dd::permission::OBJECT_RIGHTS..=dd::permission::WITH_GRANT_COLUMN_RIGHTS,
                    ) => self.recrypt(&mut data, crypto::permission_item(id, slot as u32)),
                    // The rights granted to DB:Public live in the object.
                    (Kind::Table, 12)
                    | (Kind::View, 5)
                    | (Kind::Procedure, 6)
                    | (Kind::Function, 0) => {
                        self.recrypt(&mut data, crypto::permission_item(id, slot as u32));
                    }
                    (Kind::Link, dd::link::PASSWORD) => {
                        if self.changes() {
                            crypto::wrap(self.old, self.key, &mut data);
                            crypto::wrap(self.new, self.key, &mut data);
                        }
                    }
                    (
                        Kind::Subscription,
                        dd::subscription::PASSWORD | dd::subscription::CONNECTION_STR,
                    ) => self.recrypt(&mut data, crypto::permission_item(id, slot as u32)),
                    _ => {}
                }
                (slot, data)
            })
            .collect();
        if let (Kind::Database, Some((number, engine, kind))) = (object.kind, self.database) {
            let mut put = |slot: usize, value: Vec<u8>| {
                all.retain(|(at, _)| *at != slot);
                all.push((slot, value));
            };
            put(dd::database::VERSION, number.to_le_bytes().to_vec());
            put(17, engine.to_vec());
            put(
                dd::database::DATA_ENCRYPTION_TYPE,
                kind.to_le_bytes().to_vec(),
            );
            all.sort_by_key(|(slot, _)| *slot);
        }
        all
    }

    /// A user's password secret, obfuscated for `record`: the password
    /// is lost when the cipher changes, else the secret is the same.
    fn password_secret(&self, object: &Object, secret: &[u8], record: u32) -> Vec<u8> {
        let mut data = secret.to_vec();
        crypto::obfuscate(self.old, &mut data, object.record(), 0);
        if self.changes() {
            data = self.empty_secret();
        }
        crypto::obfuscate(self.new, &mut data, record, 0);
        data
    }

    /// The secret of the empty password for the new cipher.
    fn empty_secret(&self) -> Vec<u8> {
        self.secret(b"")
    }

    /// The secret that holds `K` for `password` under the new cipher.
    fn secret(&self, password: &[u8]) -> Vec<u8> {
        let salts = login_salts();
        let mut last = [0u8; 20];
        last.copy_from_slice(self.key);
        crypto::wrap(self.new, password, &mut last);
        crypto::make_secret(self.new, password, &salts.secret_first, &last)
    }

    /// The Database object's slot 1: the table password wrapped with `K`,
    /// then the ADSSYS secret, obfuscated at the Database's record 1.
    fn admin_slot(&self, object: &Object, stored: &[u8]) -> Vec<u8> {
        if !self.changes() {
            return stored.to_vec();
        }
        let mut slot = stored.to_vec();
        crypto::obfuscate(self.old, &mut slot, object.record(), 1);
        let length = slot.first().map_or(0, |&n| usize::from(n));
        let mut table_password = slot.get(1..1 + length).unwrap_or(&[]).to_vec();
        crypto::wrap(self.old, self.key, &mut table_password);
        crypto::wrap(self.new, self.key, &mut table_password);
        let mut slot = vec![table_password.len() as u8];
        slot.extend(table_password);
        slot.extend(self.secret(self.admin));
        crypto::obfuscate(self.new, &mut slot, 1, 1);
        slot
    }
}

/// Converts the dictionary at `add` to `target`, enciphering its records
/// with the type if `encrypt`; `admin` is the ADSSYS password and `password`
/// the dictionary password an AES type takes as its key.
pub(crate) fn convert_dictionary(
    session: &Session,
    add: &Path,
    admin: &[u8],
    password: &[u8],
    target: Target,
    encrypt: bool,
) -> Result<(), Error> {
    let source = Dictionary::open(add).map_err(corrupt)?;
    let key = source.dictionary_key(admin).ok_or(Error::NotImplemented)?;
    let encrypted_tables = source.all(Kind::Table).iter().any(|table| {
        table
            .info2
            .is_some_and(|flags| flags & dd::table::flags::ENCRYPTED != 0)
    });
    if encrypted_tables {
        return Err(ace_error::sql(
            5130,
            "Cannot change database encryption type when there are encrypted tables in the \
             database.",
        ));
    }
    let mut head = [0u8; 0x174];
    std::fs::File::open(add)
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut head))
        .map_err(corrupt)?;
    let aes_layout = u32::from_le_bytes(head[0x170..0x174].try_into().unwrap()) != 0;
    let enciphered = u32::from_le_bytes(head[0x58..0x5C].try_into().unwrap())
        & tflip_storage::table::flags::ENCRYPTED
        != 0;
    // The dictionary of the default layout stays in it when it is taken to
    // an RC4 one; an AES one is enciphered in place, unless it is to keep
    // its records in the clear or to change its layout, which writes it
    // anew.
    if !aes_layout && target == Target::Rc4 {
        dd::convert::set_rc4(add, encrypt).map_err(corrupt)?;
        remake_slot0(add, &source)?;
        return rebuild_index(session, add);
    }
    let rewrite = match target {
        Target::Rc4 => aes_layout,
        Target::Aes(_) => !aes_layout || !encrypt,
    };
    let secrets = Secrets {
        old: source.scheme(),
        new: target.scheme(),
        key: &key,
        admin: &admin[..admin.len().min(20)],
        database: Some(target.database()),
    };
    // In the clear, with the new secrets: what BAK holds when it is made.
    match (aes_layout, enciphered) {
        (false, _) => dd::convert::set_rc4(add, false).map_err(corrupt)?,
        (true, true) => dd::convert::decrypt_aes(add).map_err(corrupt)?,
        (true, false) => {}
    }
    let comments = comments(add, &source)?;
    remake_in_place(add, &source, &secrets)?;
    if !rewrite {
        let Target::Aes(kind) = target else {
            return Err(Error::NotImplemented);
        };
        dd::convert::encrypt_aes(add, password, kind).map_err(corrupt)?;
        return rebuild_index(session, add);
    }
    for name in [add.to_path_buf(), dd::memo_path(add)] {
        let mut bak = name.clone().into_os_string();
        bak.push(".BAK");
        std::fs::copy(&name, bak).map_err(corrupt)?;
    }
    write_anew(
        session, add, &source, &secrets, &comments, password, target, encrypt,
    )
}

/// `sp_ModifyDatabase` of `DATA_ENCRYPTION_TYPE` (`fixtures/aes/convert`,
/// `m_*`): the type of the tables a dictionary of the default layout makes,
/// which changes the cipher of its secrets as a conversion does (the same
/// steps, in the dictionary where it is, with no layout change and no
/// `.BAK`: the users' passwords are lost, `K` stays) and the version it
/// keeps. One of the AES layout is changed the same way, its records
/// enciphered with its dictionary password as they were (`m_C_*`, `m_E_*`,
/// `m_F_*`).
pub(crate) fn set_data_encryption_type(
    session: &Session,
    conn: &connection::Connection,
    add: &Path,
    target: Target,
) -> Result<(), Error> {
    let source = Dictionary::open(add).map_err(corrupt)?;
    let key = connection_key(conn, &source).ok_or(Error::NotImplemented)?;
    let admin = conn
        .with_database_opt(|db| db.password().to_vec())
        .unwrap_or_default();
    let secrets = Secrets {
        old: source.scheme(),
        new: target.scheme(),
        key: &key,
        admin: &admin[..admin.len().min(20)],
        database: Some(target.database()),
    };
    remake_in_place(add, &source, &secrets)?;
    rebuild_dictionary(session, conn, add)
}

/// `sp_ModifyDatabase` of `ENCRYPT_TABLE_PASSWORD` (`fixtures/aes/convert`,
/// `m_*`): the table password is kept in the Database object's login slot,
/// wrapped with `K` (at most 20 bytes of it, none if it is empty) in front of
/// the administrator's secret, which stays as it is.
pub(crate) fn set_table_password(
    session: &Session,
    conn: &connection::Connection,
    add: &Path,
    password: &[u8],
) -> Result<(), Error> {
    let source = open_dict(add)?;
    let scheme = source.scheme();
    // A password cannot change under tables it enciphers, and is at least 6
    // characters long unless it is taken away (`fixtures/aes/rc4tp`).
    let encrypted_tables = source.all(Kind::Table).iter().any(|table| {
        table
            .info2
            .is_some_and(|flags| flags & dd::table::flags::ENCRYPTED != 0)
    });
    if encrypted_tables {
        return Err(ace_error::sql(
            5130,
            "Cannot change database table encryption password when there are encrypted tables \
             in the database.",
        ));
    }
    if (1..6).contains(&password.len()) {
        return Err(ace_error::sql(
            5097,
            "The database table encryption password must be at least 6 characters or longer.",
        ));
    }
    let key = connection_key(conn, &source).ok_or(Error::NotImplemented)?;
    let database = source.database().ok_or(Error::CorruptTable)?;
    let mut slot = database.slot(1).unwrap_or(&[]).to_vec();
    crypto::obfuscate(scheme, &mut slot, database.record(), 1);
    let kept = slot.first().map_or(0, |&n| 1 + usize::from(n));
    let secret = slot.get(kept..).unwrap_or(&[]).to_vec();
    let mut wrapped = password[..password.len().min(20)].to_vec();
    crypto::wrap(scheme, &key, &mut wrapped);
    let mut value = vec![wrapped.len() as u8];
    value.extend(wrapped);
    value.extend(secret);
    crypto::obfuscate(scheme, &mut value, database.record(), 1);
    let record = database.record();
    drop(source);
    // Taking the password away ends the encryption of new tables, but not
    // that of their indexes (`fixtures/aes/rc4tp`, `e8`, `e12`).
    match password.is_empty() {
        false => set_object_slot(session, conn, add, record, 1, &value),
        true => write_object_slots(
            session,
            conn,
            add,
            record,
            &[
                (1, Some(value.as_slice())),
                (dd::database::ENCRYPT_NEW_TABLE, None),
            ],
        ),
    }
}

/// Draws the Database object's login slot 0 anew, as every conversion does.
fn remake_slot0(add: &Path, source: &Dictionary) -> Result<(), Error> {
    let database = source.database().ok_or(Error::CorruptTable)?;
    let mut file = dd::DictionaryFile::open(add).map_err(corrupt)?;
    let mut record = file.record(database.record()).map_err(corrupt)?;
    dd::create::edit_properties(&mut record, &dd::memo_path(add), |slots| {
        slots[0] = Some(login_salts().slot0.to_vec());
    })
    .map_err(corrupt)?;
    file.write(database.record(), &record).map_err(corrupt)
}

/// The comments of the objects of `source`, as the memo file holds them, by
/// the objects' records.
fn comments(add: &Path, source: &Dictionary) -> Result<Vec<(u32, Vec<u8>)>, Error> {
    source
        .objects()
        .iter()
        .map(|object| {
            let bytes = dd::create::comment_bytes(add, object.record()).map_err(corrupt)?;
            Ok((object.record(), bytes))
        })
        .collect()
}

/// Gives the objects of the dictionary at `add`, in the clear, their new
/// secrets where they have them, and the Database object its new version and
/// type.
fn remake_in_place(add: &Path, source: &Dictionary, secrets: &Secrets) -> Result<(), Error> {
    let mut file = dd::DictionaryFile::open(add).map_err(corrupt)?;
    for object in source.objects() {
        let slots = secrets.slots(object, object.record());
        let mut record = file.record(object.record()).map_err(corrupt)?;
        dd::create::edit_properties(&mut record, &dd::memo_path(add), |now| {
            now.clear();
            for (slot, data) in &slots {
                if now.len() <= *slot {
                    now.resize(*slot + 1, None);
                }
                now[*slot] = Some(data.clone());
            }
        })
        .map_err(corrupt)?;
        if let (Kind::Database, Some((number, ..))) = (object.kind, secrets.database) {
            let at = dd::create::INFO1;
            record[at..at + 4].copy_from_slice(&number.to_le_bytes());
        }
        file.write(object.record(), &record).map_err(corrupt)?;
    }
    Ok(())
}

/// Writes the dictionary at `add` anew as `target`'s, from `source`: its
/// objects with the new secrets, packed into the slots from the first.
#[allow(clippy::too_many_arguments)]
fn write_anew(
    session: &Session,
    add: &Path,
    source: &Dictionary,
    secrets: &Secrets,
    comments: &[(u32, Vec<u8>)],
    password: &[u8],
    target: Target,
    encrypt: bool,
) -> Result<(), Error> {
    let next = read_next_id(add)?;
    let mut objects: Vec<&Object> = source.objects().iter().collect();
    objects.sort_by_key(|object| object.record());
    let mut kinds = Vec::new();
    {
        let mut file = dd::DictionaryFile::open(add).map_err(corrupt)?;
        for object in &objects {
            let record = file.record(object.record()).map_err(corrupt)?;
            kinds.push(
                String::from_utf8_lossy(&record[13..23])
                    .trim_end()
                    .to_owned(),
            );
        }
    }
    let memo = dd::memo_path(add);
    let salts = login_salts();
    let image = match target {
        Target::Rc4 => dd::create::new_dictionary_add(&salts),
        Target::Aes(kind) => {
            let material = tflip_storage::create::AesMaterial::generate(kind).map_err(corrupt)?;
            dd::create::new_aes_dictionary_add(
                &salts,
                &dd::create::NewAes {
                    material,
                    password,
                    encrypt: false,
                },
            )
        }
    }
    .map_err(corrupt)?;
    std::fs::write(&memo, dd::create::new_dictionary_memo()).map_err(corrupt)?;
    std::fs::write(add, &image).map_err(corrupt)?;
    if matches!(target, Target::Aes(_)) {
        // The file has a salt of its own: what the process kept of the
        // old one is another key.
        dd::forget(add);
        dd::unlock(add, password).map_err(corrupt)?;
    }
    let mut file = dd::DictionaryFile::open(add).map_err(corrupt)?;
    for (n, (object, kind)) in objects.iter().zip(&kinds).enumerate() {
        let number = n as u32 + 1;
        let slots = secrets.slots(object, number);
        let borrowed: Vec<(usize, &[u8])> = slots.iter().map(|(s, v)| (*s, v.as_slice())).collect();
        let info1 = match (object.kind, secrets.database) {
            (Kind::Database, Some((number, ..))) => number as i32,
            _ => object.info1.unwrap_or(i32::MIN),
        };
        let stored = NewObject {
            id: object.id,
            parent: object.parent as i32,
            kind,
            name: &object.name,
            info1,
            info2: object.info2.map_or(0x8000_0000, |n| n as u32),
            slots: &borrowed,
        };
        let raw = comments
            .iter()
            .find(|(record, _)| *record == object.record())
            .map(|(_, bytes)| bytes.as_slice())
            .unwrap_or_default();
        // The comment is allocated before the rest of a long property.
        let mut commented = vec![0u8; dd::create::RECORD_LENGTH];
        dd::create::set_comment_raw(&mut commented, &memo, raw).map_err(corrupt)?;
        let mut record = dd::create::object_record_continued(&stored, &memo).map_err(corrupt)?;
        let comment = dd::create::COMMENT..dd::create::COMMENT + 8;
        record[comment.clone()].copy_from_slice(&commented[comment]);
        match n {
            0 => file.write(1, &record).map_err(corrupt)?,
            _ => drop(file.insert(&record).map_err(corrupt)?),
        }
    }
    file.set_next_autoinc(next).map_err(corrupt)?;
    drop(file);
    // The dictionary is written in the clear and then enciphered where it
    // is, as the original does.
    match (target, encrypt) {
        (Target::Aes(kind), true) => {
            dd::convert::encrypt_aes(add, password, kind).map_err(corrupt)?;
        }
        (Target::Rc4, true) => dd::convert::set_rc4(add, true).map_err(corrupt)?,
        (_, false) => {}
    }
    rebuild_index(session, add)
}

/// Builds the dictionary's index anew.
fn rebuild_index(session: &Session, add: &Path) -> Result<(), Error> {
    let table = Arc::new(table::dictionary(session, add)?);
    let weights = table
        .session_settings()
        .collations()
        .ansi()
        .map(|c| c.weights);
    let collation = create::collation_crc(weights.as_ref());
    let ai_path = add.with_extension("ai");
    std::fs::remove_file(&ai_path).ok();
    index::build_dictionary_index(&table, &ai_path, collation)
}
