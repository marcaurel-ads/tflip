//! Data dictionaries (`.add`, with the memo file `.am`).
//!
//! A dictionary is a file in the ADT format (see [`crate::adt`]) with its
//! own signature, `ADS Data Dictionary`, and one record per object: the
//! database itself, its tables with their fields, index files and indexes,
//! views, procedures, functions, packages, triggers, users, groups,
//! permissions, referential integrity rules and links. Its fields:
//!
//! | Field | Type | |
//! | --- | --- | --- |
//! | `Object ID` | AutoInc | the object's number; the database is 1 |
//! | `Parent ID` | Integer | the object it belongs to: a table's fields, a user's permissions; 1 or 0 for the database's |
//! | `Object Type` | Char(10) | `Database`, `Table`, `Field`, `Index` (an index file), `Key` (an index), `View`, `StoredProc`, `Function`, `Package`, `Trigger`, `User`, `Group`, `Permission`, `Relation`, `LinkedDD` |
//! | `Object Name` | Char(200) | |
//! | `Property` | Raw(275) | the object's properties, continued in `More Property` |
//! | `More Property` | Binary | |
//! | `Info1`, `Info2` | Integer | numbers some kinds keep outside the properties |
//! | `Comment` | Memo | |
//!
//! The properties are a list of slots, each a u16 length and that many
//! bytes; a length of 0xFFFF is a slot that is not set. Which slot holds
//! what depends on the kind of object: the modules below name them, as
//! found from the fixtures and from dictionaries that differ in one property
//! each (`fixtures/dictionary/variants`). Text slots end in a NUL; those
//! that start with a UTF-8 byte order mark are UTF-8, the others are in the
//! ANSI code page. Some flags are a slot that is there, empty, or not.
//!
//! Some slots are encrypted, even in a dictionary that is not: passwords,
//! the database's login settings, a user's groups and the rights a
//! permission grants. These are read through [`crate::crypto`] (the admin or a
//! user's password unlocks the dictionary key `K`). A dictionary of AES
//! tables (`DATA_ENCRYPTION_TYPE` 5 or 6) enciphers these slots with AES-CTR
//! instead of RC4 ([`crate::crypto::Scheme`]). An encrypted dictionary, whose
//! records are enciphered as a whole, is read too: an RC4 one under the fixed
//! `AtGdQ` obfuscation key (see below), an AES one, laid out as an AES table,
//! under the key of its dictionary password (`DDPassword`).

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use crate::adt;
use crate::cp1252;
use crate::field::{Field, FieldType};
use crate::memo::MemoFile;
use crate::table::{Error as TableError, RecordCipher, RecordStatus, Records, flags};
use crate::value::Value;

pub mod convert;
mod file;
pub(crate) mod keyring;
mod memo;

pub use file::DictionaryFile;

/// The signature a dictionary starts with.
pub(crate) const SIGNATURE: &[u8] = b"ADS Data Dictionary\0";

/// The groups every dictionary has, after its own (`AdsDDFindFirstObject`
/// lists them in this order); they are not stored.
pub const BUILT_IN_GROUPS: [&str; 6] = [
    "DB:Admin",
    "DB:Backup",
    "DB:Debug",
    "DB:Public",
    "SERVER:Admin",
    "SERVER:Monitor",
];

/// The built-in group `name` names, in the case it has, if it is one.
pub fn built_in_group(name: &str) -> Option<&'static str> {
    BUILT_IN_GROUPS
        .iter()
        .copied()
        .find(|group| group.eq_ignore_ascii_case(name))
}

/// The property slot in which an object of `kind` keeps the rights granted to
/// `DB:Public` (see [`crate::crypto::public_rights_blob`]); `None` for a kind
/// that has no rights to grant. The original puts them in the object itself,
/// where other grantees' are Permission objects.
pub fn public_rights_slot(kind: Kind) -> Option<usize> {
    match kind {
        Kind::Table => Some(12),
        Kind::View => Some(5),
        Kind::Procedure => Some(6),
        Kind::Function => Some(0),
        Kind::Package | Kind::Link | Kind::Group => Some(2),
        Kind::Publication => Some(1),
        Kind::Subscription => Some(7),
        Kind::User => Some(9),
        Kind::Database => Some(16),
        _ => None,
    }
}

/// What an object's `DB:Public` rights slot holds (see
/// [`Dictionary::public_rights`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicRights {
    /// No slot, or no key to read it with: nothing is granted.
    Absent,
    Rights(u32),
    /// A slot that does not check, which the original answers with 5127.
    Invalid,
}

/// The bit that a user's built-in membership mask
/// ([`user::BUILT_IN_MEMBERSHIP`]) holds for the built-in group `name`: none for
/// `DB:Public`, whose members are everyone.
pub fn built_in_membership_bit(name: &str) -> Option<u32> {
    Some(match built_in_group(name)? {
        "DB:Backup" => 0x1,
        "SERVER:Admin" => 0x2,
        "SERVER:Monitor" => 0x4,
        "DB:Debug" => 0x1_0000,
        "DB:Admin" => 0x2000_0000,
        _ => return None,
    })
}

/// Why a dictionary could not be read.
#[derive(Debug)]
pub enum Error {
    /// The file is not a dictionary, or could not be read.
    Read(TableError),
    /// The dictionary is encrypted, and no password that opens it was given.
    Encrypted,
    /// The dictionary is encrypted with AES, and the password given is not
    /// its dictionary password.
    WrongPassword,
}

impl From<TableError> for Error {
    fn from(error: TableError) -> Self {
        Error::Read(error)
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error::Read(error.into())
    }
}

/// The kind of an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Database,
    Table,
    Field,
    /// An index file (stored as `Index`).
    IndexFile,
    /// An index, an order in an index file (stored as `Key`).
    Index,
    View,
    /// A stored procedure (stored as `StoredProc`).
    Procedure,
    Function,
    Package,
    Trigger,
    User,
    Group,
    Permission,
    /// A referential integrity rule.
    Relation,
    /// A link to another dictionary (stored as `LinkedDD`).
    Link,
    /// A replication publication (stored as `Publicatn`).
    Publication,
    /// A table published by a publication (stored as `Article`).
    Article,
    /// A replication subscription (stored as `Subscriptn`).
    Subscription,
    /// A kind tflip does not know.
    Other,
}

impl Kind {
    fn from_stored(name: &str) -> Kind {
        match name {
            "Database" => Kind::Database,
            "Table" => Kind::Table,
            "Field" => Kind::Field,
            "Index" => Kind::IndexFile,
            "Key" => Kind::Index,
            "View" => Kind::View,
            "StoredProc" => Kind::Procedure,
            "Function" => Kind::Function,
            "Package" => Kind::Package,
            "Trigger" => Kind::Trigger,
            "User" => Kind::User,
            "Group" => Kind::Group,
            "Permission" => Kind::Permission,
            "Relation" => Kind::Relation,
            "LinkedDD" => Kind::Link,
            "Publicatn" => Kind::Publication,
            "Article" => Kind::Article,
            "Subscriptn" => Kind::Subscription,
            _ => Kind::Other,
        }
    }
}

/// An object of a dictionary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub id: u32,
    /// The object it belongs to (see the module documentation).
    pub parent: u32,
    pub kind: Kind,
    pub name: String,
    /// `Info1`, `None` if NULL.
    pub info1: Option<i32>,
    /// `Info2`, `None` if NULL.
    pub info2: Option<i32>,
    pub comment: Option<String>,
    /// Whether the comment is stored with a NUL after it, as those set
    /// after the object was made are.
    pub comment_nul: bool,
    /// The object's 1-based record number in the dictionary file: it numbers
    /// the AtGdQ obfuscation of the object's encrypted property slots.
    pub(crate) record: u32,
    slots: Vec<Option<Vec<u8>>>,
}

impl Object {
    /// The object's 1-based record number (slot) in the dictionary file.
    pub fn record(&self) -> u32 {
        self.record
    }

    /// The bytes of slot `slot`, if it is set.
    pub fn slot(&self, slot: usize) -> Option<&[u8]> {
        self.slots.get(slot).and_then(|s| s.as_deref())
    }

    /// The slots that are set, with their numbers.
    pub fn slots(&self) -> impl Iterator<Item = (usize, &[u8])> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(slot, bytes)| Some((slot, bytes.as_deref()?)))
    }

    /// Whether slot `slot` is set (if only to nothing).
    pub fn has(&self, slot: usize) -> bool {
        self.slot(slot).is_some()
    }

    /// The text in slot `slot`, up to its NUL.
    pub fn text(&self, slot: usize) -> Option<String> {
        let bytes = self.slot(slot)?;
        let bytes = &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())];
        Some(match bytes.strip_prefix(b"\xEF\xBB\xBF") {
            Some(utf8) => String::from_utf8_lossy(utf8).into_owned(),
            None => cp1252::decode(bytes),
        })
    }

    /// Whether the text in slot `slot` is UTF-8 (starts with a byte order
    /// mark).
    pub fn is_utf8(&self, slot: usize) -> bool {
        self.slot(slot)
            .is_some_and(|bytes| bytes.starts_with(b"\xEF\xBB\xBF"))
    }

    /// The little-endian u16 in slot `slot`.
    pub fn u16(&self, slot: usize) -> Option<u16> {
        let bytes = self.slot(slot)?;
        Some(u16::from_le_bytes(bytes.get(..2)?.try_into().unwrap()))
    }

    /// The little-endian u32 in slot `slot`.
    pub fn u32(&self, slot: usize) -> Option<u32> {
        let bytes = self.slot(slot)?;
        Some(u32::from_le_bytes(bytes.get(..4)?.try_into().unwrap()))
    }

    /// The byte in slot `slot`.
    pub fn u8(&self, slot: usize) -> Option<u8> {
        self.slot(slot)?.first().copied()
    }
}

/// Splits properties into their slots.
pub fn slots(mut bytes: &[u8]) -> Vec<Option<Vec<u8>>> {
    let mut slots = Vec::new();
    while let [low, high, rest @ ..] = bytes {
        let length = u16::from_le_bytes([*low, *high]);
        if length == 0xFFFF {
            slots.push(None);
            bytes = rest;
            continue;
        }
        let length = usize::from(length).min(rest.len());
        slots.push(Some(rest[..length].to_vec()));
        bytes = &rest[length..];
    }
    while slots.last().is_some_and(|s| s.is_none()) {
        slots.pop();
    }
    slots
}

/// A data dictionary's objects.
#[derive(Debug, Clone)]
pub struct Dictionary {
    /// The objects, by number.
    objects: Vec<Object>,
    /// How many records the file has, deleted ones included.
    record_slots: u32,
    /// Whether the file is encrypted.
    encrypted: bool,
    /// The cipher of its encrypted slots.
    scheme: crate::crypto::Scheme,
}

impl Dictionary {
    /// Reads the dictionary at `path` and its memo file.
    pub fn open(path: &Path) -> Result<Dictionary, Error> {
        Dictionary::open_with_password(path, None)
    }

    /// Reads the dictionary at `path` and its memo file, deciphering the
    /// records of one encrypted with AES with the key of `password`, its
    /// dictionary password; without it such a dictionary is
    /// [`Error::Encrypted`], unless a connection of the process gave it.
    pub fn open_with_password(path: &Path, password: Option<&[u8]>) -> Result<Dictionary, Error> {
        let mut file = File::open(path)?;
        let structure = adt::read_structure_signed(&mut file, SIGNATURE)?;
        // An AES dictionary: its records and memo values are enciphered only
        // with `ENCRYPT TRUE`, but its dictionary password is checked even
        // when they are not, as the original checks it (7060).
        let aes = structure.header.extension != 0;
        let enciphered = structure.header.encrypted();
        // Without the password, the key a connection of the process opened
        // the dictionary with, as the original's other connections use it.
        let cipher = match (aes, password) {
            (true, Some(password)) => Some(aes_cipher(path, password)?),
            (true, None) if enciphered => Some(keyring::cipher(path).ok_or(Error::Encrypted)?),
            _ => None,
        };
        let mut memo = match structure.header.flags & flags::MEMO {
            0 => None,
            _ => {
                let mut memo = MemoFile::open(&memo_path(path))?;
                memo.set_iv_prefix(aes);
                if let (Some(cipher), true) = (&cipher, enciphered) {
                    memo.set_cipher(cipher.clone());
                } else if enciphered && matches!(structure.header.encryption_type, 2 | 3) {
                    // An RC4 dictionary's values are enciphered at the slot of
                    // their record, to their length.
                    memo.set_cipher(RecordCipher::Rc4(crate::crypto::OBFUSCATION_KEY.to_vec()));
                }
                Some(memo)
            }
        };
        let field = |name: &str| {
            structure
                .fields
                .iter()
                .find(|f| f.name == name)
                .ok_or(TableError::Corrupt("dictionary without one of its fields"))
        };
        let id = field("Object ID")?;
        let parent = field("Parent ID")?;
        let kind = field("Object Type")?;
        let name = field("Object Name")?;
        let property = field("Property")?;
        let more = field("More Property")?;
        let info1 = field("Info1")?;
        let info2 = field("Info2")?;
        let comment = field("Comment")?;

        let mut records = Records::new(file, &structure.header);
        if structure.header.encrypted() {
            // An encrypted dictionary obfuscates its records with the fixed
            // AtGdQ key, or enciphers them as an AES table's under its
            // dictionary password; its field descriptors stay in the clear.
            match structure.header.encryption_type {
                2 | 3 => {
                    records.set_cipher(RecordCipher::Rc4(crate::crypto::OBFUSCATION_KEY.to_vec()))
                }
                5 | 6 => records.set_cipher(cipher.clone().ok_or(Error::Encrypted)?),
                _ => return Err(Error::Encrypted),
            }
        }
        let mut record = vec![0; structure.header.record_length as usize];
        let mut objects = Vec::new();
        let record_slots = records.slots();
        for slot in 1..=record_slots {
            if records.status(slot)? == RecordStatus::Deleted {
                continue;
            }
            records.read(slot, &mut record)?;
            if let Some(memo) = memo.as_mut() {
                memo.decipher_record(slot);
            }
            let number = |field: &Field| match field.value(&record, None) {
                Ok(Value::Integer(n)) => Ok(Some(n)),
                Ok(Value::Null) => Ok(None),
                _ => Err(TableError::Corrupt(
                    "dictionary number field of another type",
                )),
            };
            let text = |field: &Field| {
                cp1252::decode(field.bytes(&record))
                    .trim_end_matches(['\0', ' '])
                    .to_owned()
            };
            let mut properties = property.bytes(&record).to_vec();
            if let Some(more) = more.memo_data(&record, memo.as_mut())? {
                properties.extend_from_slice(&more);
            }
            let comment = match comment.field_type {
                FieldType::Memo => match comment.value(&record, memo.as_mut())? {
                    Value::Text(text) => Some(text),
                    _ => None,
                },
                _ => None,
            };
            let comment_nul = comment.as_ref().is_some_and(|c| c.ends_with('\0'));
            let comment = comment.map(|c| c.trim_end_matches('\0').to_owned());
            let as_i32 = |n: Option<i128>| n.map(|n| n as i32);
            objects.push(Object {
                id: number(id)?.unwrap_or(0) as u32,
                parent: number(parent)?.unwrap_or(0) as u32,
                kind: Kind::from_stored(&text(kind)),
                name: text(name),
                info1: as_i32(number(info1)?),
                info2: as_i32(number(info2)?),
                comment,
                comment_nul,
                record: slot,
                slots: slots(&properties),
            });
        }
        objects.sort_by_key(|o| o.id);
        let encryption_type = objects
            .iter()
            .find(|o| o.kind == Kind::Database)
            .and_then(|o| o.u32(database::DATA_ENCRYPTION_TYPE));
        Ok(Dictionary {
            objects,
            record_slots,
            encrypted: structure.header.encrypted(),
            scheme: crate::crypto::Scheme::of_encryption_type(encryption_type),
        })
    }

    /// The cipher of the dictionary's encrypted slots.
    pub fn scheme(&self) -> crate::crypto::Scheme {
        self.scheme
    }

    /// Whether the dictionary's file is encrypted.
    pub fn encrypted(&self) -> bool {
        self.encrypted
    }

    /// The records the file has, the deleted ones among them.
    pub fn record_slots(&self) -> u32 {
        self.record_slots
    }

    /// All objects, by number.
    pub fn objects(&self) -> &[Object] {
        &self.objects
    }

    /// The object numbered `id`.
    pub fn object(&self, id: u32) -> Option<&Object> {
        self.objects
            .binary_search_by_key(&id, |o| o.id)
            .ok()
            .map(|i| &self.objects[i])
    }

    /// The database object.
    pub fn database(&self) -> Option<&Object> {
        self.objects.iter().find(|o| o.kind == Kind::Database)
    }

    /// The dictionary's table encryption password, which its encrypted tables
    /// are bound to, recovered with the admin (or a logged-in user's) password.
    /// It lives in the Database object's property slot 1, obfuscated, as
    /// `[L][table_pw wrapped with K][admin secret]`. `None` if the dictionary
    /// sets no table password or the password does not unlock it.
    pub fn table_password(&self, admin_password: &[u8]) -> Option<Vec<u8>> {
        let database = self.database()?;
        let mut slot = database.slot(1)?.to_vec();
        crate::crypto::obfuscate(self.scheme, &mut slot, database.record, 1);
        crate::crypto::dict_table_password(self.scheme, &slot, admin_password)
    }

    /// The dictionary key `K`, recovered from the Database object's login secret
    /// with `admin_password`. `K` wraps users' group memberships (property slot
    /// 1) and the table password. `None` if the password does not unlock it.
    pub fn dictionary_key(&self, admin_password: &[u8]) -> Option<Vec<u8>> {
        let database = self.database()?;
        let mut slot = database.slot(1)?.to_vec();
        crate::crypto::obfuscate(self.scheme, &mut slot, database.record, 1);
        crate::crypto::dict_key(self.scheme, &slot, admin_password)
    }

    /// The rights granted to the built-in `DB:Public` on `object`, which it keeps
    /// in a slot of its own (see [`public_rights_slot`]) deciphered with the
    /// dictionary key `key`. Without a key nothing can be read: `Absent`.
    pub fn public_rights(&self, object: &Object, key: Option<&[u8]>) -> PublicRights {
        let (Some(slot), Some(key)) = (public_rights_slot(object.kind), key) else {
            return PublicRights::Absent;
        };
        let Some(raw) = object.slot(slot) else {
            return PublicRights::Absent;
        };
        let mut blob = raw.to_vec();
        let item = crate::crypto::public_rights_item(object.id, slot as u32);
        crate::crypto::crypt(self.scheme, key, item, &mut blob);
        match crate::crypto::public_rights(&blob) {
            Some(rights) => PublicRights::Rights(rights),
            None => PublicRights::Invalid,
        }
    }

    /// The objects of kind `kind` in the order of their records, as the
    /// original engine lists them (an object stored in a deleted one's slot
    /// comes in its place: `fixtures/sql/ddtables`); fields by their number
    /// in the table.
    pub fn all(&self, kind: Kind) -> Vec<&Object> {
        let mut all: Vec<&Object> = self.objects.iter().filter(|o| o.kind == kind).collect();
        match kind {
            Kind::Field => all.sort_by_key(|o| o.info1),
            // A failed restructure puts a table's objects back in other slots
            // than they were made in; the original lists orders as made
            // (`fixtures/sql/spddalterfail`).
            Kind::Index => all.sort_by_key(|o| o.id),
            _ => all.sort_by_key(|o| o.record),
        }
        all
    }

    /// The objects of kind `kind` that belong to the object numbered
    /// `parent` (see [`Dictionary::all`] for their order).
    pub fn children(&self, parent: u32, kind: Kind) -> Vec<&Object> {
        let mut children = self.all(kind);
        children.retain(|o| o.parent == parent);
        children
    }

    /// The object of kind `kind` named `name`, whatever its case.
    pub fn named(&self, kind: Kind, name: &str) -> Option<&Object> {
        self.objects
            .iter()
            .find(|o| o.kind == kind && o.name.eq_ignore_ascii_case(name))
    }

    /// The object of kind `kind` named `name` that a statement on the
    /// database names with no package: [`Dictionary::named`], but for a
    /// function, which one of a package is not.
    pub fn named_in_database(&self, kind: Kind, name: &str) -> Option<&Object> {
        match kind {
            Kind::Function => self.child(self.database()?.id, kind, name),
            _ => self.named(kind, name),
        }
    }

    /// The object of kind `kind` named `name` that belongs to the object
    /// numbered `parent`, whatever its case.
    pub fn child(&self, parent: u32, kind: Kind, name: &str) -> Option<&Object> {
        self.objects
            .iter()
            .find(|o| o.parent == parent && o.kind == kind && o.name.eq_ignore_ascii_case(name))
    }
}

/// The record cipher of the AES dictionary at `path`: the key PBKDF2 makes
/// of `password` over the salt at 0x1A0, checked against the verifier at
/// 0x38, and the nonce at 0x194, as an AES table's. The type is the crypto
/// block's (0x190), which a dictionary that keeps its records in the clear
/// has and its header (0x4C) does not. Kept for the process's writes (see
/// [`keyring`]).
fn aes_cipher(path: &Path, password: &[u8]) -> Result<RecordCipher, Error> {
    use std::io::Read;
    if let Some(cipher) = keyring::cipher_for(path, password) {
        return Ok(cipher);
    }
    let mut head = [0u8; 0x1C0];
    File::open(path)?.read_exact(&mut head)?;
    let key_len = match head[0x190] {
        5 => 16,
        _ => 32,
    };
    let derived = crate::crypto::aes_derive(password, &head[0x1A0..0x1C0], key_len);
    if derived[key_len..key_len + 20] != head[0x38..0x4C] {
        return Err(Error::WrongPassword);
    }
    let cipher = RecordCipher::Aes {
        key: derived[..key_len].to_vec(),
        nonce: head[0x194..0x198].try_into().unwrap(),
    };
    keyring::remember(path, password, cipher.clone());
    Ok(cipher)
}

/// The bytes `add` of an AES dictionary with its records as a reader sees
/// them: deciphered with the key of `password` if they are enciphered, each
/// record's IV zeroed and its status the clear one; an RC4 dictionary's
/// records deciphered under the fixed key. What tells two such
/// dictionaries apart beyond their random IVs, as the tests compare them.
pub fn deciphered(add: &[u8], password: &[u8]) -> Vec<u8> {
    use crate::adt::{AES_IV_SIZE, RECORD_PREFIX};
    let mut bytes = add.to_vec();
    let word = |at: usize| u32::from_le_bytes(add[at..at + 4].try_into().unwrap()) as usize;
    let (data, length, slots) = (word(0x20), word(0x24), word(0x18));
    if word(0x170) == 0 {
        // An RC4 dictionary's records are enciphered under the fixed key.
        let rc4 = add[0x58] & crate::table::flags::ENCRYPTED as u8 != 0
            && add[0x4C] == adt::ENCRYPTION_RC4;
        for slot in 0..if rc4 { slots } else { 0 } {
            let record = &mut bytes[data + slot * length..data + (slot + 1) * length];
            if adt::is_enciphered(record[0]) {
                let number = slot as u32 + 1;
                let body = &mut record[crate::adt::RECORD_PREFIX..];
                crate::crypto::table_record_xor(crate::crypto::OBFUSCATION_KEY, number, body);
                record[0] = adt::clear_status(record[0]).unwrap_or(record[0]);
            }
        }
        return bytes;
    }
    let key_len = if add[0x190] == 5 { 16 } else { 32 };
    let key = &crate::crypto::aes_derive(password, &add[0x1A0..0x1C0], key_len)[..key_len];
    let nonce = &add[0x194..0x198];
    for slot in 0..slots {
        let record = &mut bytes[data + slot * length..data + (slot + 1) * length];
        let fields = RECORD_PREFIX + AES_IV_SIZE;
        if adt::is_enciphered(record[0]) {
            let iv: [u8; 8] = record[RECORD_PREFIX..fields].try_into().unwrap();
            crate::crypto::aes_ctr_xor(key, &iv, nonce.try_into().unwrap(), &mut record[fields..]);
            record[0] = adt::clear_status(record[0]).unwrap_or(record[0]);
        }
        record[RECORD_PREFIX..fields].fill(0);
    }
    bytes
}

/// Finishes the index at `ai_path` of the dictionary at `add_path`, just
/// built in the clear: one that encrypts its records with AES enciphers its
/// index's tree pages and marks it, as the original keeps it
/// (`fixtures/aes/dictwrite`); any other's stays as it is.
pub fn finish_index(add_path: &Path, ai_path: &Path) -> Result<(), Error> {
    use std::io::Read;
    let mut head = [0u8; 0x174];
    File::open(add_path)?.read_exact(&mut head)?;
    let aes = u32::from_le_bytes(head[0x170..0x174].try_into().unwrap()) != 0;
    let encrypted = u32::from_le_bytes(head[0x58..0x5C].try_into().unwrap()) & flags::ENCRYPTED;
    let corrupt = |_| Error::Read(TableError::Corrupt("dictionary index"));
    // An RC4 dictionary's index is enciphered under the fixed key.
    if !aes && encrypted != 0 && head[0x4C] == adt::ENCRYPTION_RC4 {
        crate::adi::encipher_rc4(ai_path, crate::crypto::OBFUSCATION_KEY).map_err(corrupt)?;
        return Ok(());
    }
    if !aes || encrypted == 0 {
        return Ok(());
    }
    let Some((RecordCipher::Aes { key, nonce }, counter)) = keyring::reserve(add_path)? else {
        return Err(Error::Encrypted);
    };
    crate::adi::encipher_tree_pages(ai_path, &key, &nonce, &counter).map_err(corrupt)?;
    crate::adi::make_aes(ai_path, &head[0x38..0x4C])?;
    keyring::persist(add_path, &counter)?;
    Ok(())
}

/// Runs `work` on the index at `ai_path` of the dictionary at `add_path` in
/// the clear, and enciphers it again after as the original keeps it, when the
/// dictionary encrypts its records with AES: only the pages `work` changed
/// are enciphered again, each under a new IV. `work` just runs for any other
/// dictionary. [`Error::Encrypted`] if the process holds no key.
pub fn with_clear_index<T>(
    add_path: &Path,
    ai_path: &Path,
    work: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    use std::io::Read;
    let mut head = [0u8; 0x174];
    File::open(add_path)?.read_exact(&mut head)?;
    let aes = u32::from_le_bytes(head[0x170..0x174].try_into().unwrap()) != 0;
    let encrypted = u32::from_le_bytes(head[0x58..0x5C].try_into().unwrap()) & flags::ENCRYPTED;
    if !aes || encrypted == 0 {
        return work();
    }
    let Some((RecordCipher::Aes { key, nonce }, counter)) = keyring::reserve(add_path)? else {
        return Err(Error::Encrypted);
    };
    let done = crate::adi::with_clear(
        ai_path,
        &key,
        &nonce,
        &counter,
        &head[0x38..0x4C],
        work,
        || Error::Read(TableError::Corrupt("dictionary index")),
    );
    keyring::persist(add_path, &counter)?;
    done
}

/// Whether the dictionary at `add_path` encrypts its records with AES, and so
/// its index is kept current through [`with_clear_index`].
pub fn aes_index(add_path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 0x174];
    let Ok(mut file) = File::open(add_path) else {
        return false;
    };
    file.read_exact(&mut head).is_ok()
        && u32::from_le_bytes(head[0x170..0x174].try_into().unwrap()) != 0
        && u32::from_le_bytes(head[0x58..0x5C].try_into().unwrap()) & flags::ENCRYPTED != 0
}

/// Whether the dictionary at `add_path` enciphers its index file, which is
/// then rebuilt whole rather than kept current.
pub fn encrypts_index(add_path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 0x174];
    let Ok(mut file) = File::open(add_path) else {
        return true;
    };
    if file.read_exact(&mut head).is_err() {
        return true;
    }
    u32::from_le_bytes(head[0x58..0x5C].try_into().unwrap()) & flags::ENCRYPTED != 0
}

/// Gives the process the dictionary password of the AES dictionary at
/// `path`, which its writes then use ([`Error::WrongPassword`] if it is not
/// the dictionary's).
pub fn unlock(path: &Path, password: &[u8]) -> Result<(), Error> {
    aes_cipher(path, password).map(|_| ())
}

/// Forgets the key the process holds of the AES dictionary at `path`: its
/// file was written anew, with a salt of its own.
pub fn forget(path: &Path) {
    keyring::forget(path)
}

/// The memo file of the dictionary at `path`: the same name with `.am`
/// (`.AM` if the dictionary's extension is upper case).
pub fn memo_path(path: &Path) -> PathBuf {
    let upper = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e == "ADD");
    path.with_extension(if upper { "AM" } else { "am" })
}

/// Writing a new dictionary. A dictionary is an ADT table (see the module
/// documentation) with one record per object; a fresh one holds a single
/// Database object (record 1). This builds that `.add` file; its memo file
/// is [`create::adm_bytes`]`(8)`, and its index (`.ai`, the dictionary's
/// system orders) is built on top by the engine.
/// How a table's field descriptors are enciphered, as its header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureCipher {
    /// RC4, with the 20 bytes of the header's password check.
    Rc4([u8; 20]),
    /// AES.
    Aes,
}

pub mod create {
    use std::fs::OpenOptions;
    use std::io::{self, Read, Seek, SeekFrom, Write};
    use std::path::Path;

    use super::{SIGNATURE, database};
    use crate::adt::{self, HEADER_SIZE};
    use crate::create::{FieldSpec, adm_bytes, adt_bytes};
    use crate::crypto;
    use crate::field::FieldType;
    use crate::table::Error;

    /// A dictionary's fixed record length (its nine-field schema).
    pub const RECORD_LENGTH: usize = 524;

    /// A dictionary object to serialize into a record.
    pub struct NewObject<'a> {
        pub id: u32,
        pub parent: i32,
        /// The object type, as [`super::Kind`] stores it (`Table`, `Field`, …).
        pub kind: &'a str,
        pub name: &'a str,
        pub info1: i32,
        pub info2: u32,
        /// The set property slots, as `(index, bytes)`; the rest are unset.
        pub slots: &'a [(usize, &'a [u8])],
    }

    /// The record bytes of one dictionary object: an active row with the
    /// object's id, parent, type, name, property slots and info numbers. The
    /// type and name are space-padded as ADT Character fields are.
    pub fn object_record(object: &NewObject) -> Vec<u8> {
        let mut record = vec![0u8; RECORD_LENGTH];
        record[0] = adt::ACTIVE;
        let put_str = |r: &mut [u8], at: usize, s: &[u8], size: usize| {
            r[at..at + size].fill(b' ');
            let n = s.len().min(size);
            r[at..at + n].copy_from_slice(&s[..n]);
        };
        record[5..9].copy_from_slice(&object.id.to_le_bytes());
        record[9..13].copy_from_slice(&object.parent.to_le_bytes());
        put_str(&mut record, 13, object.kind.as_bytes(), 10);
        put_str(&mut record, 23, object.name.as_bytes(), 200);
        record[223..223 + 275].copy_from_slice(&property(object.slots, 275));
        record[507..511].copy_from_slice(&object.info1.to_le_bytes());
        record[511..515].copy_from_slice(&object.info2.to_le_bytes());
        record
    }

    /// Inserts object `records` into the dictionary `.add` at `path` as the
    /// original engine stores the objects of a table `CREATE TABLE` adds:
    /// each in the slot at the head of the deleted records' free list, or
    /// after the last (`fixtures/sql/ddtables`). `next_autoinc` is the id
    /// the next object will take.
    pub fn insert_objects(
        path: &Path,
        records: &[Vec<u8>],
        next_autoinc: u32,
    ) -> Result<(), Error> {
        let mut file = super::DictionaryFile::open(path)?;
        for record in records {
            file.insert(record)?;
        }
        file.set_next_autoinc(next_autoinc)
    }

    /// Binds the `.adt` table at `path` to the dictionary it joined, as the
    /// original engine does when a table is added (`AdsDDAddTable`, `CREATE
    /// TABLE`). The engine stamps the table's own file with 2 at header
    /// offset `0x14` (1 for a free table), its dictionary object `id` at
    /// `0x160` (0 for a free table) and the dictionary's file name as a
    /// back-reference path (`".\\" + name`, NUL-terminated, at `0x5c`).
    /// Without this the original engine refuses to open the table through
    /// the dictionary (error 7074, its stored id not matching the
    /// dictionary's).
    pub fn bind_table(path: &Path, id: u32, dictionary_file: &str) -> io::Result<()> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        // A table laid out for AES keeps its mark, 3 (`fixtures/aes/dictwrite`).
        let mut extension = [0u8; 4];
        file.seek(SeekFrom::Start(0x170))?;
        file.read_exact(&mut extension)?;
        let mark: u32 = match u32::from_le_bytes(extension) {
            0 => 2,
            _ => 3,
        };
        file.seek(SeekFrom::Start(0x14))?;
        file.write_all(&mark.to_le_bytes())?;
        let back_reference = format!(".\\{dictionary_file}\0");
        file.seek(SeekFrom::Start(0x5C))?;
        file.write_all(back_reference.as_bytes())?;
        file.seek(SeekFrom::Start(0x160))?;
        file.write_all(&id.to_le_bytes())?;
        Ok(())
    }

    /// Whether the `.adt` table at `path` is bound to a dictionary, as
    /// [`bind_table`] marks it; the original marks an AES dictionary's table 3, not 2.
    pub fn is_bound(path: &Path) -> io::Result<bool> {
        let mut file = std::fs::File::open(path)?;
        let mut mark = [0u8; 4];
        file.seek(SeekFrom::Start(0x14))?;
        file.read_exact(&mut mark)?;
        Ok(u32::from_le_bytes(mark) >= 2)
    }

    /// How the structure of the `.adt` table at `path` is enciphered, read
    /// from its header: `None` if it is in the clear.
    pub fn structure_cipher(path: &Path) -> io::Result<Option<super::StructureCipher>> {
        let mut file = std::fs::File::open(path)?;
        let mut header = [0u8; 0x5C];
        file.read_exact(&mut header)?;
        let flags = u32::from_le_bytes(header[0x58..0x5C].try_into().unwrap());
        if flags & crate::table::flags::ENCRYPTED_DESCRIPTORS == 0 {
            return Ok(None);
        }
        Ok(Some(match header[0x4C] {
            5 | 6 => super::StructureCipher::Aes,
            _ => super::StructureCipher::Rc4(header[0x38..0x4C].try_into().unwrap()),
        }))
    }

    /// Frees the `.adt` table at `path` from the dictionary it belonged to, the
    /// reverse of [`bind_table`] (`AdsDDFreeTable`): its object id goes back to
    /// a free table's (1 at `0x14`, 0 at `0x160`) and the dictionary
    /// back-reference path at `0x5c` is cleared, as the original engine does.
    pub fn free_table(path: &Path) -> io::Result<()> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        // The back-reference path at 0x5c, up to its NUL, to clear.
        let mut back = [0u8; 256];
        file.seek(SeekFrom::Start(0x5C))?;
        let read = file.read(&mut back)?;
        let len = back[..read]
            .iter()
            .position(|&b| b == 0)
            .map_or(read, |n| n + 1);
        file.seek(SeekFrom::Start(0x14))?;
        file.write_all(&1u32.to_le_bytes())?;
        file.seek(SeekFrom::Start(0x5C))?;
        file.write_all(&vec![0u8; len])?;
        file.seek(SeekFrom::Start(0x160))?;
        file.write_all(&0u32.to_le_bytes())?;
        Ok(())
    }

    /// Gives a table laid out for AES the mark, 3, a freed one of the other
    /// layout does not have: the `.BAK` an `ALTER TABLE` of an encrypted
    /// table leaves keeps it (`fixtures/aes/dictwrite`,
    /// `44_restructure_one`).
    pub fn keep_aes_mark(path: &Path) -> io::Result<()> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut extension = [0u8; 4];
        file.seek(SeekFrom::Start(0x170))?;
        file.read_exact(&mut extension)?;
        if u32::from_le_bytes(extension) != 0 {
            file.seek(SeekFrom::Start(0x14))?;
            file.write_all(&3u32.to_le_bytes())?;
        }
        Ok(())
    }

    /// Points the bound `.adt` table at `path` at its dictionary by another
    /// path (`reference`, from the table's folder), as the original engine
    /// does when the table is moved.
    pub fn set_back_reference(path: &Path, reference: &str) -> io::Result<()> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut back = [0u8; 256];
        file.seek(SeekFrom::Start(0x5C))?;
        let read = file.read(&mut back)?;
        let len = back[..read]
            .iter()
            .position(|&b| b == 0)
            .map_or(read, |n| n + 1);
        file.seek(SeekFrom::Start(0x5C))?;
        file.write_all(&vec![0u8; len])?;
        file.seek(SeekFrom::Start(0x5C))?;
        file.write_all(format!("{reference}\0").as_bytes())?;
        Ok(())
    }

    /// The record offset of the object's name: 200 bytes, space-padded.
    pub const NAME: usize = 23;

    /// The record offset of the object's `Info1` number.
    pub const INFO1: usize = 507;

    /// The record offset of the object's `Info2` number (a table's flags).
    pub const INFO2: usize = 511;

    /// The record offset of the `Comment` memo field's reference: a u32
    /// block number then a u32 length.
    pub const COMMENT: usize = 515;

    /// The record offset of the `More Property` memo field's reference: a u32
    /// block number then a u32 length.
    pub const MORE_PROPERTY: usize = 498;

    /// The record of a new object as [`object_record`] makes it, but with
    /// properties too long for the `Property` field: as the original engine
    /// does, the field holds their first [`PROPERTY_LENGTH`] bytes and the
    /// rest goes into the dictionary memo file at `memo_path`, referred to by
    /// the `More Property` field.
    pub fn object_record_continued(object: &NewObject, memo_path: &Path) -> Result<Vec<u8>, Error> {
        object_record_continued_as(object, memo_path, true)
    }

    /// [`object_record_continued`] for a rule of referential integrity: the
    /// original engine counts one allocation in the memo file for each of the
    /// rule's messages that lies in the continuation whole, besides the one
    /// that begins in the `Property` field, and none twice for its size
    /// (`fixtures/sql/spddrilong`).
    pub fn object_record_continued_once(
        object: &NewObject,
        memo_path: &Path,
    ) -> Result<Vec<u8>, Error> {
        let record = object_record_continued_as(object, memo_path, false)?;
        let last = object.slots.iter().map(|(i, _)| *i).max().unwrap_or(0);
        let mut at = 0;
        let mut whole = 0;
        for index in 0..=last {
            match object.slots.iter().find(|(i, _)| *i == index) {
                Some((_, data)) => {
                    whole += usize::from(at >= PROPERTY_LENGTH);
                    at += 2 + data.len();
                }
                None => at += 2,
            }
        }
        if whole > 0 {
            let mut memo = super::memo::DictMemo::open(memo_path)?;
            let allocator = &mut memo;
            for _ in 0..whole {
                allocator.count_allocation()?;
            }
        }
        Ok(record)
    }

    /// The record of an object whose properties the original engine sets one
    /// at a time, in the order of `order`, rewriting the whole blob after
    /// each: the continuation in the memo file is allocated when the blob
    /// first overflows, and moved whenever a later slot changes the blocks it
    /// takes, leaving what it held behind (`fixtures/dictionary/proclong`).
    pub fn object_record_stepwise(
        object: &NewObject,
        memo_path: &Path,
        order: &[usize],
    ) -> Result<Vec<u8>, Error> {
        let mut record = object_record(&NewObject {
            id: object.id,
            parent: object.parent,
            kind: object.kind,
            name: object.name,
            info1: object.info1,
            info2: object.info2,
            slots: &[],
        });
        for &index in order {
            let Some((_, data)) = object.slots.iter().find(|(i, _)| *i == index) else {
                continue;
            };
            edit_properties(&mut record, memo_path, |slots| {
                if slots.len() <= index {
                    slots.resize(index + 1, None);
                }
                slots[index] = Some(data.to_vec());
            })?;
        }
        Ok(record)
    }

    fn object_record_continued_as(
        object: &NewObject,
        memo_path: &Path,
        adding: bool,
    ) -> Result<Vec<u8>, Error> {
        let length = property_length(object.slots);
        if length <= PROPERTY_LENGTH {
            return Ok(object_record(object));
        }
        let mut record = object_record(&NewObject {
            id: object.id,
            parent: object.parent,
            kind: object.kind,
            name: object.name,
            info1: object.info1,
            info2: object.info2,
            slots: &[],
        });
        let blob = property(object.slots, length);
        record[PROPERTY..PROPERTY + PROPERTY_LENGTH].copy_from_slice(&blob[..PROPERTY_LENGTH]);
        let (block, stored) = continue_properties(&blob[PROPERTY_LENGTH..], memo_path, adding)?;
        record[MORE_PROPERTY..MORE_PROPERTY + 4].copy_from_slice(&block.to_le_bytes());
        record[MORE_PROPERTY + 4..MORE_PROPERTY + 8].copy_from_slice(&stored.to_le_bytes());
        Ok(record)
    }

    /// Stores the part of an object's properties that does not fit its
    /// `Property` field in the dictionary memo file at `memo_path`; the block
    /// it is in and its length, for the `More Property` field. `adding` is
    /// whether the object is new: only then does the original engine count a
    /// large continuation twice.
    fn continue_properties(
        rest: &[u8],
        memo_path: &Path,
        adding: bool,
    ) -> Result<(u32, u32), Error> {
        let stored =
            u32::try_from(rest.len()).map_err(|_| Error::Corrupt("properties too long"))?;
        let mut memo = super::memo::DictMemo::open(memo_path)?;
        let allocator = &mut memo;
        let block = allocator.alloc(stored)?;
        // The original engine counts a new object's continuation larger than
        // a slab's chunks (32 blocks) twice among the memo file's allocations.
        if adding && allocator.blocks(stored) > crate::adm::MAX_CHUNK {
            allocator.count_allocation()?;
        }
        allocator.write(block, rest)?;
        Ok((block, stored))
    }

    /// Edits the properties of the object `record` holds, as the original
    /// engine does when a setter changes them: `edit` gets the positional
    /// slots (see [`decode_property`]), the continuation in `More Property`
    /// included, and what it leaves is stored again, continued in the memo
    /// file at `memo_path` if it is longer than [`PROPERTY_LENGTH`].
    pub fn edit_properties(
        record: &mut [u8],
        memo_path: &Path,
        edit: impl FnOnce(&mut Vec<Option<Vec<u8>>>),
    ) -> Result<(), Error> {
        let word = |at: usize| u32::from_le_bytes(record[at..at + 4].try_into().unwrap());
        let (old_block, old_length) = (word(MORE_PROPERTY), word(MORE_PROPERTY + 4));
        let mut blob = record[PROPERTY..PROPERTY + PROPERTY_LENGTH].to_vec();
        if old_block != 0 {
            let mut memo = super::memo::DictMemo::open(memo_path)?;
            blob.extend(memo.read_raw(old_block, old_length as usize)?);
        }
        let mut slots = decode_property(&blob);
        edit(&mut slots);
        let pairs: Vec<(usize, &[u8])> = slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_deref().map(|b| (i, b)))
            .collect();
        let length = property_length(&pairs);
        let blob = property(&pairs, length.max(PROPERTY_LENGTH));
        record[PROPERTY..PROPERTY + PROPERTY_LENGTH].copy_from_slice(&blob[..PROPERTY_LENGTH]);
        let (block, stored) = match length > PROPERTY_LENGTH {
            true => {
                let rest = &blob[PROPERTY_LENGTH..length];
                let memo = super::memo::DictMemo::open(memo_path)?;
                let allocator = &memo;
                let same = old_block != 0
                    && allocator.blocks(old_length) == allocator.blocks(rest.len() as u32);
                drop(memo);
                match same {
                    // The same blocks hold it: written over, not allocated.
                    true => {
                        let mut memo = super::memo::DictMemo::open(memo_path)?;
                        let allocator = &mut memo;
                        allocator.write(old_block, rest)?;
                        (old_block, rest.len() as u32)
                    }
                    // The old block is freed first, so a continuation that
                    // stays long is stored where it was if it still fits.
                    false => {
                        free_comment(memo_path, old_block, old_length)?;
                        continue_properties(rest, memo_path, false)?
                    }
                }
            }
            // A property that fits again leaves the block it was continued
            // in allocated, as the original engine does.
            false => (0, 0),
        };
        record[MORE_PROPERTY..MORE_PROPERTY + 4].copy_from_slice(&block.to_le_bytes());
        record[MORE_PROPERTY + 4..MORE_PROPERTY + 8].copy_from_slice(&stored.to_le_bytes());
        Ok(())
    }

    /// Takes the properties of the object `record` holds out of the record
    /// and frees their continuation in the memo file, as the original engine
    /// does of a Field object it makes anew from another after a
    /// restructure, which stores them again (`fixtures/sql/spddlongalter`);
    /// [`store_properties`] puts them back.
    pub fn release_properties(
        record: &mut [u8],
        memo_path: &Path,
    ) -> Result<Vec<Option<Vec<u8>>>, Error> {
        let word = |at: usize| u32::from_le_bytes(record[at..at + 4].try_into().unwrap());
        let (block, length) = (word(MORE_PROPERTY), word(MORE_PROPERTY + 4));
        let mut blob = record[PROPERTY..PROPERTY + PROPERTY_LENGTH].to_vec();
        if block != 0 {
            let mut memo = super::memo::DictMemo::open(memo_path)?;
            blob.extend(memo.read_raw(block, length as usize)?);
            memo.free(block, length)?;
        }
        record[PROPERTY..PROPERTY + PROPERTY_LENGTH].fill(0);
        record[MORE_PROPERTY..MORE_PROPERTY + 8].fill(0);
        Ok(decode_property(&blob))
    }

    /// Stores the `slots` [`release_properties`] gave back, with `edit`
    /// applied, continued in the memo file where they now fit.
    pub fn store_properties(
        record: &mut [u8],
        memo_path: &Path,
        slots: Vec<Option<Vec<u8>>>,
        edit: impl FnOnce(&mut Vec<Option<Vec<u8>>>),
    ) -> Result<(), Error> {
        edit_properties(record, memo_path, |now| {
            *now = slots;
            edit(now);
        })
    }

    /// Stores `comment` (its bytes and a NUL terminator) in the dictionary
    /// memo file at `memo_path` as the original engine stores an object's
    /// comment, and sets the reference in `record`'s `Comment` field. An
    /// empty comment writes nothing and leaves the reference zero.
    pub fn set_comment(record: &mut [u8], memo_path: &Path, comment: &[u8]) -> Result<(), Error> {
        if comment.is_empty() {
            return Ok(());
        }
        let mut value = comment.to_vec();
        value.push(0);
        let stored = u32::try_from(value.len()).map_err(|_| Error::Corrupt("comment too long"))?;
        let mut memo = super::memo::DictMemo::open(memo_path)?;
        let allocator = &mut memo;
        let block = allocator.alloc(stored)?;
        allocator.write(block, &value)?;
        record[COMMENT..COMMENT + 4].copy_from_slice(&block.to_le_bytes());
        record[COMMENT + 4..COMMENT + 8].copy_from_slice(&stored.to_le_bytes());
        Ok(())
    }

    /// Writes `comment` (and its NUL) over the one at `block`, of `length`
    /// bytes, if it takes the same number of blocks: the length it now has,
    /// or `None` when the old comment has to go and a new one be stored
    /// (`fixtures/sql/spddvprops`).
    pub fn rewrite_comment(
        memo_path: &Path,
        block: u32,
        length: u32,
        comment: &[u8],
    ) -> Result<Option<u32>, Error> {
        if block == 0 || comment.is_empty() {
            return Ok(None);
        }
        let mut value = comment.to_vec();
        value.push(0);
        let stored = u32::try_from(value.len()).map_err(|_| Error::Corrupt("comment too long"))?;
        let mut memo = super::memo::DictMemo::open(memo_path)?;
        let allocator = &mut memo;
        if allocator.blocks(length) != allocator.blocks(stored) {
            return Ok(None);
        }
        allocator.write(block, &value)?;
        Ok(Some(stored))
    }

    /// Writes `comment` over the one at `block`, of `length` bytes, if it
    /// takes the same number of blocks, as [`rewrite_comment`] does, but as
    /// [`set_comment_strlen`] stores it: the bare text. The length it now
    /// has, or `None` when a new one has to be stored
    /// (`fixtures/sql/ddfuncs`).
    pub fn rewrite_comment_strlen(
        memo_path: &Path,
        block: u32,
        length: u32,
        comment: &[u8],
    ) -> Result<Option<u32>, Error> {
        if block == 0 || comment.is_empty() {
            return Ok(None);
        }
        let stored =
            u32::try_from(comment.len()).map_err(|_| Error::Corrupt("comment too long"))?;
        let mut memo = super::memo::DictMemo::open(memo_path)?;
        let allocator = &mut memo;
        if allocator.blocks(length) != allocator.blocks(stored) {
            return Ok(None);
        }
        allocator.write(block, comment)?;
        Ok(Some(stored))
    }

    /// Stores the `value` an object's comment held, as it was stored (the NUL
    /// among it, or not), and sets the reference in `record`'s `Comment`
    /// field: a dictionary written anew from another keeps each comment as
    /// it was.
    pub fn set_comment_raw(record: &mut [u8], memo_path: &Path, value: &[u8]) -> Result<(), Error> {
        if value.is_empty() {
            return Ok(());
        }
        let stored = u32::try_from(value.len()).map_err(|_| Error::Corrupt("comment too long"))?;
        let mut memo = super::memo::DictMemo::open(memo_path)?;
        let allocator = &mut memo;
        let block = allocator.alloc(stored)?;
        allocator.write(block, value)?;
        record[COMMENT..COMMENT + 4].copy_from_slice(&block.to_le_bytes());
        record[COMMENT + 4..COMMENT + 8].copy_from_slice(&stored.to_le_bytes());
        Ok(())
    }

    /// Stores `comment` in the dictionary memo as [`set_comment`] does, but
    /// records the length without a NUL terminator, as `AdsDDAddProcedure`
    /// stores a procedure's comment (the memo block is zero-filled, so a reader
    /// still finds a terminator past the stored bytes). An empty comment writes
    /// nothing and leaves the reference zero.
    pub fn set_comment_strlen(
        record: &mut [u8],
        memo_path: &Path,
        comment: &[u8],
    ) -> Result<(), Error> {
        if comment.is_empty() {
            return Ok(());
        }
        let stored =
            u32::try_from(comment.len()).map_err(|_| Error::Corrupt("comment too long"))?;
        let mut memo = super::memo::DictMemo::open(memo_path)?;
        let allocator = &mut memo;
        let block = allocator.alloc(stored)?;
        allocator.write(block, comment)?;
        record[COMMENT..COMMENT + 4].copy_from_slice(&block.to_le_bytes());
        record[COMMENT + 4..COMMENT + 8].copy_from_slice(&stored.to_le_bytes());
        Ok(())
    }

    /// What the original engine does to the comment of an object it makes
    /// anew from another (a table's Field objects after a restructure,
    /// `fixtures/sql/ddalter`): the comment stays in its block, and the
    /// record gives its length without the NUL, which counts as one more
    /// allocation of the memo file. An object with no comment is left alone.
    pub fn renew_comment(record: &mut [u8], memo_path: &Path) -> Result<(), Error> {
        let word = |at: usize| u32::from_le_bytes(record[at..at + 4].try_into().unwrap());
        let (block, length) = (word(COMMENT), word(COMMENT + 4));
        if block == 0 {
            return Ok(());
        }
        let mut memo = super::memo::DictMemo::open(memo_path)?;
        let text = memo.read_raw(block, length as usize)?;
        let stored = text.iter().position(|&b| b == 0).unwrap_or(text.len()) as u32;
        memo.count_allocation()?;
        record[COMMENT + 4..COMMENT + 8].copy_from_slice(&stored.to_le_bytes());
        Ok(())
    }

    /// Whether the record of an object continues its properties in the memo
    /// file.
    pub fn has_more_properties(record: &[u8]) -> bool {
        record[MORE_PROPERTY..MORE_PROPERTY + 4] != [0; 4]
    }

    /// Counts `count` more allocations in the dictionary memo file at
    /// `memo_path`, as the original engine does for objects it makes anew
    /// whose properties continue there.
    pub fn count_allocations(memo_path: &Path, count: usize) -> Result<(), Error> {
        if count == 0 {
            return Ok(());
        }
        let mut memo = super::memo::DictMemo::open(memo_path)?;
        for _ in 0..count {
            memo.count_allocation()?;
        }
        Ok(())
    }

    /// Reads the `Comment` field reference (block number and length) of the
    /// object at the 1-based `record` (as [`super::Object::record`] gives it)
    /// from the dictionary `.add` at `path`. A zero block means the object
    /// has no comment or description.
    pub fn comment_ref(path: &Path, record: u32) -> io::Result<(u32, u32)> {
        let record = super::DictionaryFile::open(path)
            .and_then(|mut file| file.record(record))
            .map_err(|error| io::Error::other(format!("{error:?}")))?;
        let word = |at: usize| u32::from_le_bytes(record[at..at + 4].try_into().unwrap());
        Ok((word(COMMENT), word(COMMENT + 4)))
    }

    /// The comment of the object at the 1-based `record` of the dictionary
    /// `.add` at `path`, as its memo file holds it: the bytes its length
    /// counts, a NUL among them if the comment was stored with one. Empty
    /// for an object with none.
    pub fn comment_bytes(path: &Path, record: u32) -> Result<Vec<u8>, Error> {
        let (block, length) = comment_ref(path, record).map_err(|_| Error::Corrupt("comment"))?;
        if block == 0 {
            return Ok(Vec::new());
        }
        let mut memo = super::memo::DictMemo::open(&super::memo_path(path))?;
        memo.read_raw(block, length as usize)
    }

    /// Frees the memo blocks of the object at the 1-based `record` of the
    /// dictionary `.add` at `path` when it is deleted: its `More Property`
    /// continuation, then its comment or description, as the original engine
    /// does (`fixtures/dictionary/removelong`). A zero block refers to nothing.
    pub fn free_object_memos(path: &Path, record: u32) -> Result<(), Error> {
        let refs = super::DictionaryFile::open(path)?.record(record)?;
        let refs = &refs[MORE_PROPERTY..COMMENT + 8];
        let word = |at: usize| u32::from_le_bytes(refs[at..at + 4].try_into().unwrap());
        let memo_path = crate::dd::memo_path(path);
        free_comment(&memo_path, word(0), word(4))?;
        free_comment(
            &memo_path,
            word(COMMENT - MORE_PROPERTY),
            word(COMMENT - MORE_PROPERTY + 4),
        )
    }

    /// Frees the memo block a `Comment` field refers to, given the `block` and
    /// `length` stored in the record (a u32 block number and its length), as the
    /// original engine frees an object's description or comment memo when the
    /// object is deleted. A zero block refers to nothing and is left alone.
    pub fn free_comment(memo_path: &Path, block: u32, length: u32) -> Result<(), Error> {
        if block == 0 {
            return Ok(());
        }
        let mut memo = super::memo::DictMemo::open(memo_path)?;
        let allocator = &mut memo;
        allocator.free(block, length)?;
        Ok(())
    }

    /// Stores a group's or user's `description` in the dictionary memo at
    /// `memo_path` and sets the reference in `record`'s `Comment` field, as the
    /// original engine stores it when a user or group is created
    /// (`AdsDDCreateUserGroup`, `AdsDDCreateUser`). Unlike a comment — which is
    /// stored with a terminating NUL counted in its length — a description is
    /// stored as its bare text: the memo block is allocated and the length kept
    /// in the record are both the text's own length, and the rest of the last
    /// block stays the zero fill of a freshly grown memo. An empty description
    /// writes nothing and leaves the reference zero.
    pub fn set_description(
        record: &mut [u8],
        memo_path: &Path,
        description: &[u8],
    ) -> Result<(), Error> {
        if description.is_empty() {
            return Ok(());
        }
        let stored =
            u32::try_from(description.len()).map_err(|_| Error::Corrupt("description too long"))?;
        let mut memo = super::memo::DictMemo::open(memo_path)?;
        let allocator = &mut memo;
        let block = allocator.alloc(stored)?;
        allocator.write(block, description)?;
        record[COMMENT..COMMENT + 4].copy_from_slice(&block.to_le_bytes());
        record[COMMENT + 4..COMMENT + 8].copy_from_slice(&stored.to_le_bytes());
        Ok(())
    }

    /// The 9 fields every dictionary has, in order (see the module docs).
    fn dictionary_fields() -> [FieldSpec; 9] {
        let f = |name: &str, field_type, length| FieldSpec {
            name: name.to_owned(),
            field_type,
            length,
            decimals: 0,
            compressed: false,
        };
        [
            f("Object ID", FieldType::AutoInc, 0),
            f("Parent ID", FieldType::Integer, 0),
            f("Object Type", FieldType::Character, 10),
            f("Object Name", FieldType::Character, 200),
            f("Property", FieldType::Raw, 275),
            f("More Property", FieldType::Binary, 0),
            f("Info1", FieldType::Integer, 0),
            f("Info2", FieldType::Integer, 0),
            f("Comment", FieldType::Memo, 0),
        ]
    }

    /// The random material a new dictionary needs. The engine fills the
    /// Database object's login slots with randomly-salted secrets; it
    /// re-derives whatever it checks from the stored salts, so any values
    /// serve. `secret_first` and `secret_last` salt the `ADSSYS` login secret
    /// (slot 1, the one the engine authenticates against); `slot0` and
    /// `slot4` are the other two encrypted login slots (the admin secret and
    /// the "login not required" marker).
    pub struct NewDictRandom {
        pub secret_first: [u8; 20],
        pub secret_last: [u8; 20],
        pub slot0: [u8; 20],
        pub slot4: [u8; 20],
    }

    /// What of a slot's bytes the blob keeps: the original stores the length
    /// as 16 bits and as many bytes as that, so the 65,536 of a trigger's
    /// script of 65,535 are none (`fixtures/sql/trigbig/`).
    fn kept(data: &[u8]) -> &[u8] {
        &data[..data.len() as u16 as usize]
    }

    /// Serialize an object's property blob into `len` bytes: each set slot is
    /// a little-endian u16 length and its bytes; an unset slot is `0xFFFF`,
    /// which is the `0xFF` fill the field starts as.
    fn property(slots: &[(usize, &[u8])], len: usize) -> Vec<u8> {
        let mut bytes = vec![0xFF; len];
        let last = slots.iter().map(|(i, _)| *i).max().unwrap_or(0);
        let mut at = 0;
        for index in 0..=last {
            if let Some((_, data)) = slots.iter().find(|(i, _)| *i == index) {
                let data = kept(data);
                bytes[at..at + 2].copy_from_slice(&(data.len() as u16).to_le_bytes());
                bytes[at + 2..at + 2 + data.len()].copy_from_slice(data);
                at += 2 + data.len();
            } else {
                at += 2;
            }
        }
        bytes
    }

    /// The bytes [`property`] serializes `slots` into; more than
    /// [`PROPERTY_LENGTH`] go on into `More Property` (see
    /// [`object_record_continued`]).
    pub fn property_length(slots: &[(usize, &[u8])]) -> usize {
        let last = slots.iter().map(|(i, _)| *i).max().unwrap_or(0);
        let data: usize = slots.iter().map(|(_, data)| kept(data).len()).sum();
        2 * (last + 1) + data
    }

    /// The record offset and length of the `Property` field: the per-object
    /// property blob (the format [`property`] serializes).
    pub const PROPERTY: usize = 223;
    pub const PROPERTY_LENGTH: usize = 275;

    /// Decodes a property blob (see [`property`]) into its positional slots:
    /// `Some(bytes)` for a set slot, `None` for an unset one. Trailing unset
    /// slots are dropped, so re-encoding the result reproduces the blob.
    pub fn decode_property(blob: &[u8]) -> Vec<Option<Vec<u8>>> {
        let mut out: Vec<Option<Vec<u8>>> = Vec::new();
        let mut at = 0;
        while at + 2 <= blob.len() {
            let len = u16::from_le_bytes([blob[at], blob[at + 1]]);
            if len == 0xFFFF {
                out.push(None);
                at += 2;
            } else {
                let len = len as usize;
                if at + 2 + len > blob.len() {
                    break;
                }
                out.push(Some(blob[at + 2..at + 2 + len].to_vec()));
                at += 2 + len;
            }
        }
        while matches!(out.last(), Some(None)) {
            out.pop();
        }
        out
    }

    /// Sets slot `index` of a property blob to `value`, re-encoding the whole
    /// blob in place as the engine stores it (a read-modify-write of one
    /// object property, preserving the object's other slots). Errors if the
    /// result would not fit in `blob`.
    pub fn set_property_slot(blob: &mut [u8], index: usize, value: &[u8]) -> Result<(), Error> {
        put_property_slot(blob, index, Some(value))
    }

    /// Unsets slot `index` of a property blob (a slot the engine stores by its
    /// presence, such as a field's `NOT_NULL` marker), re-encoding the blob in
    /// place and preserving the object's other slots. A slot already unset is
    /// left as it is.
    pub fn unset_property_slot(blob: &mut [u8], index: usize) -> Result<(), Error> {
        put_property_slot(blob, index, None)
    }

    /// Sets slot `index` to `Some(value)` or unsets it with `None`, re-encoding
    /// the blob in place (see [`set_property_slot`] and [`unset_property_slot`]).
    fn put_property_slot(blob: &mut [u8], index: usize, value: Option<&[u8]>) -> Result<(), Error> {
        let mut slots = decode_property(blob);
        if index >= slots.len() {
            if value.is_none() {
                return Ok(());
            }
            slots.resize_with(index + 1, || None);
        }
        slots[index] = value.map(<[u8]>::to_vec);
        let pairs: Vec<(usize, &[u8])> = slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_deref().map(|b| (i, b)))
            .collect();
        // Measure the encoding first, so an overrun is an error, not a panic.
        let last = pairs.iter().map(|(i, _)| *i).max().unwrap_or(0);
        let mut needed = 0;
        for i in 0..=last {
            needed += 2 + pairs
                .iter()
                .find(|(j, _)| *j == i)
                .map_or(0, |(_, d)| kept(d).len());
        }
        if needed > blob.len() {
            return Err(Error::Corrupt("property blob too large"));
        }
        blob.copy_from_slice(&property(&pairs, blob.len()));
        Ok(())
    }

    /// The `.add` bytes of a new, empty data dictionary: the 9-field schema
    /// and one Database object (record 1), with the login secrets built from
    /// `random`. The memo file is [`adm_bytes`]`(8)`.
    ///
    /// The Database object's login slot 1 carries the `ADSSYS` secret the
    /// engine authenticates against, AtGdQ-obfuscated as the record stores it;
    /// the default password is empty. Slots 0 and 4 (the admin secret and the
    /// login-not-required marker) are encrypted and only their presence
    /// matters, so they take `random`'s bytes as-is.
    pub fn new_dictionary_add(random: &NewDictRandom) -> Result<Vec<u8>, Error> {
        new_dictionary(random, 2, b"6.2\0", 3)
    }

    /// The `.add` of a new dictionary that encrypts its records with RC4
    /// (`AdsDDCreate` of `usEncrypt` 1): that of [`new_dictionary_add`], its
    /// header holding the password check of the fixed `AtGdQ` key, the
    /// encryption type 3 and the encrypted flag, and its one record enciphered
    /// under that key (`fixtures/aes/dictwrite`, `22_rc4_create`).
    pub fn new_rc4_dictionary_add(random: &NewDictRandom) -> Result<Vec<u8>, Error> {
        let mut bytes = new_dictionary_add(random)?;
        let key = crypto::OBFUSCATION_KEY;
        bytes[0x38..0x4C].copy_from_slice(&crypto::rc4_password_check(key));
        bytes[0x4C..0x50].copy_from_slice(&3u32.to_le_bytes());
        let table_flags = u32::from_le_bytes(bytes[0x58..0x5C].try_into().unwrap());
        let table_flags = table_flags | crate::table::flags::ENCRYPTED;
        bytes[0x58..0x5C].copy_from_slice(&table_flags.to_le_bytes());
        let data = u32::from_le_bytes(bytes[0x20..0x24].try_into().unwrap()) as usize;
        bytes[data] = adt::ENCRYPTED_ACTIVE;
        crypto::table_record_xor(key, 1, &mut bytes[data + adt::RECORD_PREFIX..]);
        Ok(bytes)
    }

    /// The `.add` bytes of a new data dictionary of `version`, which an engine
    /// of `min_engine` or later reads, its tables encrypted with
    /// `encryption` (`DATA_ENCRYPTION_TYPE`); see [`new_dictionary_add`].
    fn new_dictionary(
        random: &NewDictRandom,
        version: u32,
        min_engine: &[u8],
        encryption: u32,
    ) -> Result<Vec<u8>, Error> {
        let fields = dictionary_fields();
        let mut bytes = adt_bytes(&fields)?;
        // A dictionary's signature replaces the table one; its header then
        // holds one record and the next object id (2, the Database took 1).
        bytes[..SIGNATURE.len()].copy_from_slice(SIGNATURE);
        bytes[0x18..0x1C].copy_from_slice(&1u32.to_le_bytes()); // record slots
        bytes[0x50..0x54].copy_from_slice(&2u32.to_le_bytes()); // next AutoInc

        // The Database object's login secret, obfuscated as the record stores
        // it (record 1, slot 1 -> item 2).
        let scheme = crypto::Scheme::of_encryption_type(Some(encryption));
        let mut secret_slot =
            crypto::make_admin_slot(scheme, b"", &random.secret_first, &random.secret_last);
        crypto::obfuscate(scheme, &mut secret_slot, 1, 1);

        let version = version.to_le_bytes();
        let encryption = encryption.to_le_bytes();
        let empty: &[u8] = &[];
        let slots: &[(usize, &[u8])] = &[
            (0, &random.slot0),
            (1, &secret_slot),
            (database::LOGIN_NOT_REQUIRED, &random.slot4),
            (database::RIGHTS_NOT_CHECKED, empty),
            (database::VERSION, &version),
            (17, min_engine),
            (database::DATA_ENCRYPTION_TYPE, &encryption),
        ];

        // The one record: an active row holding the Database object.
        debug_assert_eq!(HEADER_SIZE + fields.len() * adt::DESCRIPTOR_SIZE, 2200);
        debug_assert_eq!(
            u32::from_le_bytes(bytes[0x24..0x28].try_into().unwrap()) as usize,
            RECORD_LENGTH
        );
        let record = object_record(&NewObject {
            id: 1,
            parent: 0,
            kind: "Database",
            name: "Database",
            info1: u32::from_le_bytes(version) as i32,
            info2: 0x8000_0000,
            slots,
        });
        bytes.extend_from_slice(&record);
        Ok(bytes)
    }

    /// Arms the table at `path`, laid out for AES and bound to the AES
    /// dictionary at `add_path` but not yet encrypted, as the original arms a
    /// table its dictionary makes after `ENCRYPT_NEW_TABLE`
    /// (`fixtures/aes/dictwrite`, `07_newtable`): the dictionary's type, salt
    /// and so its key and verifier, a nonce and descriptor IV of its own and
    /// the IV counter a reserved block past it, its records and descriptors
    /// enciphered (it has no records yet). A connection of the process must
    /// have given the dictionary's password.
    pub fn arm_table(path: &Path, add_path: &Path) -> Result<(), Error> {
        use crate::table::flags;
        let Some(crate::table::RecordCipher::Aes { key, .. }) = super::keyring::cipher(add_path)
        else {
            return Err(Error::Encrypted);
        };
        let mut dictionary = [0u8; 0x1C0];
        std::fs::File::open(add_path)?.read_exact(&mut dictionary)?;
        let mut material = crate::create::AesMaterial::generate(dictionary[0x190])?;
        material.salt = dictionary[0x1A0..0x1C0].try_into().unwrap();
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut header = vec![0u8; adt::HEADER_SIZE];
        file.read_exact(&mut header)?;
        let word =
            |header: &[u8], at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
        let (data, extension) = (word(&header, 0x20) as usize, word(&header, 0x170) as usize);
        if extension != crate::create::AES_EXTENSION {
            return Err(Error::Corrupt("not a table laid out for AES"));
        }
        header[0x38..0x4C].copy_from_slice(&dictionary[0x38..0x4C]);
        header[0x4C] = material.enc_type;
        let table_flags = word(&header, 0x58) | flags::ENCRYPTED | flags::ENCRYPTED_DESCRIPTORS;
        header[0x58..0x5C].copy_from_slice(&table_flags.to_le_bytes());
        let mut block = vec![0u8; extension];
        block[0..4].copy_from_slice(&u32::from(material.enc_type).to_le_bytes());
        block[4..8].copy_from_slice(&material.nonce);
        let counter = material.iv_counter + super::keyring::RESERVED;
        block[8..16].copy_from_slice(&counter.to_le_bytes());
        block[16..48].copy_from_slice(&material.salt);
        block[48..56].copy_from_slice(&material.descriptor_iv);
        let start = adt::HEADER_SIZE + extension;
        let mut descriptors = vec![0u8; data - start];
        file.seek(SeekFrom::Start(start as u64))?;
        file.read_exact(&mut descriptors)?;
        crypto::aes_ctr_xor(
            &key,
            &material.descriptor_iv,
            &material.nonce,
            &mut descriptors,
        );
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header)?;
        file.write_all(&block)?;
        file.write_all(&descriptors)?;
        Ok(())
    }

    /// Arms the new, empty table at `path`, bound to a dictionary of RC4
    /// tables that has the table password `key`, as the original arms a table
    /// its dictionary makes after `ENCRYPT_NEW_TABLE` (`fixtures/aes/rc4tp`):
    /// the password check and type 3, the encrypted flags, and the field
    /// descriptors (the whole region up to the first record) enciphered as
    /// one stream.
    pub fn arm_table_rc4(path: &Path, key: &[u8]) -> Result<(), Error> {
        use crate::table::flags;
        let key = &key[..key.len().min(crypto::KEY_MAX)];
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut header = vec![0u8; adt::HEADER_SIZE];
        file.read_exact(&mut header)?;
        let word =
            |header: &[u8], at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
        let data = word(&header, 0x20) as usize;
        let fields = u16::from_le_bytes(header[0x166..0x168].try_into().unwrap());
        header[0x38..0x4C].copy_from_slice(&crypto::rc4_password_check(key));
        header[0x4C] = 3;
        let table_flags = word(&header, 0x58) | flags::ENCRYPTED | flags::ENCRYPTED_DESCRIPTORS;
        header[0x58..0x5C].copy_from_slice(&table_flags.to_le_bytes());
        let mut region = vec![0u8; data - adt::HEADER_SIZE];
        file.read_exact(&mut region)?;
        crypto::rc4_crypt(key, 0x45 * u32::from(fields), &mut region);
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header)?;
        file.write_all(&region)?;
        Ok(())
    }

    /// What makes a new dictionary an AES one (`CREATE DATABASE …
    /// ENCRYPTIONTYPE AES128 | AES256 DDPASSWORD p [ENCRYPT TRUE]`): the
    /// crypto block's random material, of the type its tables take, the
    /// dictionary password, and whether it encrypts its records.
    pub struct NewAes<'a> {
        pub material: crate::create::AesMaterial,
        pub password: &'a [u8],
        pub encrypt: bool,
    }

    /// The `.add` bytes of a new AES data dictionary, as the original makes
    /// one (`fixtures/aes/dictwrite`, `00_create`): the bytes of
    /// [`new_dictionary_add`] for version 5 (`10.1`) with the AES type, laid
    /// out as an AES table's (the 600-byte crypto block, the verifier of the
    /// dictionary password, each field 8 bytes on, after the record's IV,
    /// the descriptors in the clear), its one record enciphered if it
    /// encrypts its records. The IV counter is as the original leaves it:
    /// one past the descriptor IV, and a reserved block further in one that
    /// encrypts its records.
    pub fn new_aes_dictionary_add(random: &NewDictRandom, aes: &NewAes) -> Result<Vec<u8>, Error> {
        use crate::adt::{AES_IV_SIZE, DESCRIPTOR_SIZE, RECORD_PREFIX};
        let material = &aes.material;
        let plain = new_dictionary(random, 5, b"10.1\0", u32::from(material.enc_type))?;
        let fields = dictionary_fields().len();
        let descriptors = HEADER_SIZE..HEADER_SIZE + fields * DESCRIPTOR_SIZE;
        let key_len = material.key_len();
        let derived = crypto::aes_derive(aes.password, &material.salt, key_len);

        let mut bytes = plain[..HEADER_SIZE].to_vec();
        let extension = crate::create::AES_EXTENSION;
        let data = HEADER_SIZE + extension + fields * DESCRIPTOR_SIZE;
        let put = |bytes: &mut Vec<u8>, at: usize, value: u32| {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        };
        bytes[0x14] = 3;
        put(&mut bytes, 0x20, data as u32);
        put(&mut bytes, 0x24, (RECORD_LENGTH + AES_IV_SIZE) as u32);
        bytes[0x38..0x4C].copy_from_slice(&derived[key_len..key_len + 20]);
        if aes.encrypt {
            bytes[0x4C] = material.enc_type;
            let table_flags = u32::from_le_bytes(bytes[0x58..0x5C].try_into().unwrap());
            put(
                &mut bytes,
                0x58,
                table_flags | crate::table::flags::ENCRYPTED,
            );
        }
        bytes[0x164] = 12;
        put(&mut bytes, 0x170, extension as u32);

        let record_iv = material.iv_counter;
        let mut block = vec![0u8; extension];
        block[0..4].copy_from_slice(&u32::from(material.enc_type).to_le_bytes());
        block[4..8].copy_from_slice(&material.nonce);
        // One past the descriptor IV, and a block reserved past that in one
        // that encrypts its records.
        let counter = match aes.encrypt {
            true => material.iv_counter + super::keyring::RESERVED,
            false => material.iv_counter,
        };
        block[8..16].copy_from_slice(&counter.to_le_bytes());
        block[16..48].copy_from_slice(&material.salt);
        block[48..56].copy_from_slice(&material.descriptor_iv);
        bytes.extend_from_slice(&block);

        for descriptor in plain[descriptors.clone()].chunks(DESCRIPTOR_SIZE) {
            let mut descriptor = descriptor.to_vec();
            let offset = u16::from_le_bytes([descriptor[131], descriptor[132]]);
            let offset = offset + AES_IV_SIZE as u16;
            descriptor[131..133].copy_from_slice(&offset.to_le_bytes());
            bytes.extend_from_slice(&descriptor);
        }

        let record = &plain[descriptors.end..descriptors.end + RECORD_LENGTH];
        let mut wide = record[..RECORD_PREFIX].to_vec();
        wide.extend_from_slice(&[0; AES_IV_SIZE]);
        wide.extend_from_slice(&record[RECORD_PREFIX..]);
        if aes.encrypt {
            let key = &derived[..key_len];
            crate::adt::aes_encipher_record(
                key,
                &material.nonce,
                &record_iv.to_le_bytes(),
                &mut wide,
            );
        }
        bytes.extend_from_slice(&wide);
        Ok(bytes)
    }

    /// The `.add` the original leaves when it refuses the password of an AES
    /// dictionary it was creating (`fixtures/aes/dictwrite`, `14_ddcreate`):
    /// the header of [`new_aes_dictionary_add`] and its descriptors, with no
    /// key material, no verifier, no record and the count of the next record
    /// at 1.
    pub fn unkeyed_aes_dictionary_add(random: &NewDictRandom, kind: u8) -> Result<Vec<u8>, Error> {
        let material = crate::create::AesMaterial::generate(kind)?;
        let aes = NewAes {
            material,
            password: b"",
            encrypt: false,
        };
        let mut bytes = new_aes_dictionary_add(random, &aes)?;
        let data = u32::from_le_bytes(bytes[0x20..0x24].try_into().unwrap()) as usize;
        bytes.truncate(data);
        bytes[0x18..0x1C].fill(0);
        bytes[0x38..0x4C].fill(0);
        bytes[0x50..0x54].copy_from_slice(&1u32.to_le_bytes());
        bytes[0x190..0x1C8].fill(0);
        Ok(bytes)
    }

    /// The memo file of a new dictionary.
    pub fn new_dictionary_memo() -> Vec<u8> {
        adm_bytes(8)
    }
}

/// The database's slots. `Info1` is the dictionary's version, as
/// [`database::VERSION`].
pub mod database {
    pub const DEFAULT_TABLE_PATH: usize = 2;
    pub const TEMP_TABLE_PATH: usize = 3;
    /// Set (encrypted) unless users must log in.
    pub const LOGIN_NOT_REQUIRED: usize = 4;
    /// Set (empty) unless access rights are checked.
    pub const RIGHTS_NOT_CHECKED: usize = 5;
    /// Set (empty) if new tables are encrypted.
    pub const ENCRYPT_NEW_TABLE: usize = 6;
    /// u16: 1 if users may connect over the Internet.
    pub const ENABLE_INTERNET: usize = 7;
    /// u16; 2 if not set.
    pub const INTERNET_SECURITY_LEVEL: usize = 8;
    /// u16; 5 if not set.
    pub const MAX_FAILED_ATTEMPTS: usize = 9;
    /// u32: the dictionary's version (2, or 4 since web delta tables).
    pub const VERSION: usize = 11;
    /// u16.
    pub const VERSION_MAJOR: usize = 12;
    /// u16.
    pub const VERSION_MINOR: usize = 13;
    /// u16: 1 if logins are disabled.
    pub const LOGINS_DISABLED: usize = 14;
    pub const LOGINS_DISABLED_MESSAGE: usize = 15;
    pub const USER_DEFINED: usize = 18;
    pub const FTS_DELIMITERS: usize = 19;
    pub const FTS_NOISE: usize = 20;
    pub const FTS_DROP_CHARS: usize = 21;
    pub const FTS_CONDITIONAL_CHARS: usize = 22;
    /// Set (empty) if the indexes of encrypted tables are encrypted.
    pub const ENCRYPT_INDEXES: usize = 23;
    pub const QUERY_LOG_TABLE: usize = 24;
    /// Set (empty) if communication is encrypted.
    pub const ENCRYPT_COMMUNICATION: usize = 25;
    /// Set (empty) if DLL caching is disabled.
    pub const DISABLE_DLL_CACHING: usize = 26;
    /// Set (empty) if every trigger of the dictionary is disabled.
    pub const TRIGGERS_DISABLED: usize = 27;
    /// u32: the encryption type of tables (3 is RC4).
    pub const DATA_ENCRYPTION_TYPE: usize = 28;
    /// u32.
    pub const QUERY_VIA_ROOT: usize = 29;
    /// u16: 1 if the maximum of failed logins is enforced.
    pub const ENFORCE_MAX_FAILED_LOGINS: usize = 30;
}

/// A table's slots. `Info1` is the number of its primary key's index (0
/// for none), `Info2` its [`table::flags`].
pub mod table {
    /// The table's file, relative to the dictionary's folder.
    pub const RELATIVE_PATH: usize = 0;
    /// Set (empty) unless the table is created when it is missing.
    pub const NO_AUTO_CREATE: usize = 1;
    pub const VALIDATION_EXPR: usize = 2;
    pub const VALIDATION_MSG: usize = 3;
    /// u16.
    pub const FIELD_COUNT: usize = 4;
    /// u32: the number of the referential integrity rule the table is in.
    pub const RI_GRAPH: usize = 5;
    /// u16.
    pub const MEMO_BLOCK_SIZE: usize = 7;
    /// u32: the Key object that is a DBF table's primary key (an ADT table's
    /// is in its `Info1`, which a DBF table's keeps its type in;
    /// `fixtures/sql/ddbf`).
    pub const PRIMARY_KEY: usize = 13;
    pub const DEFAULT_INDEX: usize = 8;
    /// u32.
    pub const TRIGGER_TYPES: usize = 9;
    /// u32.
    pub const TRIGGER_OPTIONS: usize = 10;
    /// Set (empty) if the table's triggers are disabled.
    pub const TRIGGERS_DISABLED: usize = 11;
    pub const USER_DEFINED: usize = 19;

    /// The bits of a table's `Info2`.
    pub mod flags {
        /// The table is encrypted (a table an AES dictionary encrypts as it
        /// makes it: `fixtures/aes/dictwrite`, and the encrypted tables of
        /// `fixtures/aes`).
        pub const ENCRYPTED: i32 = 0x01;
        /// The table's character type is `ADS_OEM`, not `ADS_ANSI`; only a
        /// DBF table keeps it (`fixtures/dictionary/dbfcorners`).
        pub const OEM: i32 = 0x02;
        /// Permission level 1 (2 if neither bit is set).
        pub const PERMISSION_LEVEL_1: i32 = 0x04;
        /// Permission level 3.
        pub const PERMISSION_LEVEL_3: i32 = 0x08;
        pub const TRANSACTION_FREE: i32 = 0x10;
        pub const WEB_DELTA: i32 = 0x20;
        pub const CONCURRENCY: i32 = 0x40;
        /// The caching mode is the two bits from here.
        pub const CACHING_SHIFT: u32 = 29;
    }
}

/// A field's slots. `Info1` is its number in the table.
pub mod field {
    pub const DEFAULT_VALUE: usize = 0;
    /// Set (empty) if the field cannot be NULL.
    pub const NOT_NULL: usize = 1;
    pub const MIN_VALUE: usize = 2;
    pub const MAX_VALUE: usize = 3;
    pub const VALIDATION_MSG: usize = 4;
    /// Three u16: the type, length and decimals.
    pub const DEFINITION: usize = 5;
    /// u32.
    pub const OPTIONS: usize = 6;
    pub const USER_DEFINED: usize = 19;
}

/// An index file's slots. `Info1` is its type (6, ADI).
pub mod index_file {
    /// The file, relative to the dictionary's folder.
    pub const RELATIVE_PATH: usize = 0;
    /// u32.
    pub const PAGE_SIZE: usize = 2;
}

/// An index's slots. `Info1` is the number of its index file.
pub mod index {
    pub const EXPRESSION: usize = 0;
    pub const CONDITION: usize = 1;
    /// u32: the `ADS_*` index options; the original engine reports a
    /// unique index as `ADS_CANDIDATE` too.
    pub const OPTIONS: usize = 2;
    /// u16.
    pub const KEY_LENGTH: usize = 3;
    /// u16.
    pub const KEY_TYPE: usize = 4;
    /// u16.
    pub const FTS_MIN_LENGTH: usize = 5;
    pub const FTS_DELIMITERS: usize = 6;
    pub const FTS_NOISE: usize = 7;
    pub const FTS_DROP_CHARS: usize = 8;
    pub const FTS_CONDITIONAL_CHARS: usize = 9;
    pub const COLLATION: usize = 10;
}

/// A view's slots.
pub mod view {
    pub const STATEMENT: usize = 0;
    /// u16.
    pub const STATEMENT_LENGTH: usize = 1;
    /// Set (empty) if the view's triggers are disabled.
    pub const TRIGGERS_DISABLED: usize = 4;
}

/// A stored procedure's slots.
pub mod procedure {
    pub const INPUT: usize = 0;
    pub const OUTPUT: usize = 1;
    pub const DLL_NAME: usize = 2;
    pub const DLL_FUNCTION_NAME: usize = 3;
    /// u32.
    pub const INVOKE_OPTION: usize = 4;
    pub const SCRIPT: usize = 5;
}

/// A user group's slots.
pub mod group {
    /// The group's user-defined property (`ADS_DD_USER_DEFINED_PROP`).
    pub const USER_DEFINED: usize = 14;
}

/// A function's slots. Its parent is its package, or the database;
/// `Info2` is the number of its parameters.
pub mod function {
    pub const RETURN_TYPE: usize = 4;
    pub const PARAMETERS: usize = 5;
    pub const SCRIPT: usize = 6;
}

/// A trigger's slots. `Info1` is the number of its table (1, the
/// database's, for a database trigger).
pub mod trigger {
    /// u32.
    pub const EVENT_TYPE: usize = 0;
    /// u32.
    pub const TRIGGER_TYPE: usize = 1;
    /// u32.
    pub const CONTAINER_TYPE: usize = 2;
    /// The script, or the DLL.
    pub const CONTAINER: usize = 3;
    pub const FUNCTION_NAME: usize = 4;
    /// u32.
    pub const PRIORITY: usize = 5;
    /// u32.
    pub const OPTIONS: usize = 6;
    /// Set (empty) if the trigger is disabled.
    pub const TRIGGERS_DISABLED: usize = 15;
}

/// A user's slots. `Info1` is the number of groups the user is in, which
/// slot 1 lists encrypted.
pub mod user {
    /// The login secret, obfuscated at the user's record (slot 0): the block
    /// that authenticates the user's password.
    pub const PASSWORD_SECRET: usize = 0;
    /// The user's group membership, encrypted with the dictionary key: the
    /// member group object ids as little-endian `u32`s, in membership order.
    /// Absent for a user in no group but the built-in `DB:Public`.
    pub const GROUP_MEMBERSHIP: usize = 1;
    /// The user's membership in the built-in groups but `DB:Public`, 16 bytes
    /// enciphered under the dictionary key (see
    /// [`built_in_membership_blob`](crate::crypto::built_in_membership_blob)).
    /// Present from the first such membership on, and kept when the last is
    /// taken away.
    pub const BUILT_IN_MEMBERSHIP: usize = 8;
    /// u16: 1 if the user may connect over the Internet.
    pub const ENABLE_INTERNET: usize = 4;
    /// u16: 1 if the user's logins are disabled.
    pub const LOGINS_DISABLED: usize = 7;
    /// Set (empty) unless changing the password needs the old one.
    pub const NO_OLD_PASSWORD: usize = 10;
    pub const USER_DEFINED: usize = 16;
}

/// A Permission object's slots. Its `Parent` is the grantee's object id, its
/// `Object Name` the target object's id as uppercase 8-digit hex, and `Info1`
/// the target id. The rights live in the property slots, enciphered.
pub mod permission {
    /// The object rights (a `u32` mask with its CRC), enciphered.
    pub const OBJECT_RIGHTS: usize = 1;
    /// The per-column rights (one byte per field in field order, with a CRC),
    /// enciphered.
    pub const COLUMN_RIGHTS: usize = 2;
    /// The object rights granted with the grant option (the "second grant kind"),
    /// enciphered. A revoke writes it (zero without grant options).
    pub const WITH_GRANT_RIGHTS: usize = 3;
    /// The per-column rights granted with the grant option (one byte per field,
    /// with a CRC), enciphered — the grant-option counterpart of
    /// [`COLUMN_RIGHTS`].
    pub const WITH_GRANT_COLUMN_RIGHTS: usize = 4;
    /// The `ADS_PERMISSION_*` rights bits an access-rights string can name.
    pub const READ: u32 = 0x0001;
    pub const UPDATE: u32 = 0x0002;
    pub const EXECUTE: u32 = 0x0004;
    pub const INSERT: u32 = 0x0010;
    pub const DELETE: u32 = 0x0020;
    /// `ADS_PERMISSION_INHERIT`: set in a user's rights mask (a user also
    /// inherits its groups' rights), not in a group's.
    pub const INHERIT: u32 = 0x08;
    /// The rights bits a table grant may carry (`ADS_PERMISSION_ALL` on a table
    /// expands to these): read, update, insert, delete, alter and drop.
    pub const TABLE_RIGHTS: u32 = 0x0001 | 0x0002 | 0x0010 | 0x0020 | 0x0100 | 0x0200;
    /// `ADS_PERMISSION_WITH_GRANT`: accepted on a grant but not kept in the
    /// object-rights slot.
    pub const WITH_GRANT: u32 = 0x8000_0000;

    /// The byte a column's rights are kept as in the per-column slots: read
    /// `0x01`, update `0x02` and insert `0x04` (not its mask bit `0x10`).
    pub fn column_byte(rights: u32) -> u8 {
        let bit = |right: u32, byte: u8| if rights & right != 0 { byte } else { 0 };
        bit(READ, 0x01) | bit(UPDATE, 0x02) | bit(INSERT, 0x04)
    }

    /// The rights a per-column byte holds (the inverse of [`column_byte`]).
    pub fn column_rights(byte: u8) -> u32 {
        let bit = |b: u8, right: u32| if byte & b != 0 { right } else { 0 };
        bit(0x01, READ) | bit(0x02, UPDATE) | bit(0x04, INSERT)
    }

    /// A user's or group's rights to create objects: property slot 6 of its User
    /// or Group object, one byte per kind of object (see [`create_index`]) and a
    /// CRC-32, enciphered as a Permission object's column rights are (at
    /// `crypto::permission_item(id, 6)`). A user's bytes start as
    /// [`CREATE_INHERIT`], a group's as zero; the slot is written by the first
    /// change.
    pub const CREATE_RIGHTS: usize = 6;
    /// The number of bytes in [`CREATE_RIGHTS`].
    pub const CREATE_RIGHTS_LENGTH: usize = 18;
    /// In a create-rights byte: the right to create.
    pub const CREATE: u8 = 0x80;
    /// In a create-rights byte: the right to create, with the grant option.
    pub const CREATE_WITH_GRANT: u8 = 0x40;
    /// In a create-rights byte: the user inherits its groups' right to create.
    pub const CREATE_INHERIT: u8 = 0x08;
    /// In a create-rights byte: the inherit bit with the grant option.
    pub const CREATE_INHERIT_WITH_GRANT: u8 = 0x20;

    /// The byte of [`CREATE_RIGHTS`] that holds the right to create objects of
    /// the `ADS_DD_*_OBJECT` type `object_type`, for the kinds that have one: a
    /// type's own number, except functions (18) at 7 and packages (19) at 4.
    pub fn create_index(object_type: u16) -> Option<usize> {
        match object_type {
            1 | 6 | 8 | 9 | 10 | 12 | 15 | 17 => Some(object_type.into()),
            18 => Some(7),
            19 => Some(4),
            _ => None,
        }
    }
}

/// A referential integrity rule's slots.
pub mod relation {
    /// u32: the number of the parent table.
    pub const PRIMARY_TABLE: usize = 0;
    /// u32: the number of the parent table's index.
    pub const PRIMARY_INDEX: usize = 1;
    /// u32: the number of the child table.
    pub const FOREIGN_TABLE: usize = 2;
    /// u32: the number of the child table's index.
    pub const FOREIGN_INDEX: usize = 3;
    /// u8.
    pub const UPDATE_RULE: usize = 4;
    /// u8.
    pub const DELETE_RULE: usize = 5;
    pub const NO_PKEY_ERROR: usize = 6;
    pub const CASCADE_ERROR: usize = 7;
}

/// A link's slots. `Info1` is its options.
pub mod link {
    pub const USER_NAME: usize = 0;
    /// The link user's password, `wrap`ped with the dictionary key (its bytes and
    /// a NUL terminator).
    pub const PASSWORD: usize = 1;
    /// The linked dictionary, relative to this one's folder.
    pub const RELATIVE_PATH: usize = 4;
}

/// A replication publication's slots. Its parent is the Database.
pub mod publication {
    /// u32: the publication options (`ADS_DD_PUBLICATION_OPTIONS`).
    pub const OPTIONS: usize = 0;
    /// The user-defined property (`ADS_DD_USER_DEFINED_PROP`).
    pub const USER_DEFINED: usize = 4;
}

/// An article's slots. Its parent is its publication, its `Object Name` the
/// published table's object id as 8-digit hex, and `Info1` that table id.
pub mod article {
    /// The horizontal (row) filter (`ADS_DD_ARTICLE_FILTER`).
    pub const FILTER: usize = 0;
    /// The row-identifying columns, a `;`-terminated list
    /// (`ADS_DD_ARTICLE_ID_COLUMNS`).
    pub const ID_COLUMNS: usize = 1;
    /// The filter with field names rewritten to `F<number>`
    /// (`ADS_DD_ARTICLE_FILTER_SHORT`).
    pub const FILTER_SHORT: usize = 2;
    /// The include or exclude column list (a `;`-terminated list); which it is
    /// is told by [`LIST_KIND`].
    pub const COLUMN_LIST: usize = 3;
    /// u16: 1 if [`COLUMN_LIST`] is the include list (`ADS_DD_ARTICLE_INCLUDE_COLUMNS`),
    /// 2 if it is the exclude list (`ADS_DD_ARTICLE_EXCLUDE_COLUMNS`).
    pub const LIST_KIND: usize = 4;
    /// Set (empty) if inserts use SQL MERGE (`ADS_DD_ARTICLE_INSERT_MERGE`).
    pub const INSERT_MERGE: usize = 5;
    /// Set (empty) if updates use SQL MERGE (`ADS_DD_ARTICLE_UPDATE_MERGE`).
    pub const UPDATE_MERGE: usize = 6;
    /// [`LIST_KIND`] value for an include list.
    pub const LIST_INCLUDE: u16 = 1;
    /// [`LIST_KIND`] value for an exclude list.
    pub const LIST_EXCLUDE: u16 = 2;
}

/// A replication subscription's slots. Its parent is the Database; `Info1` is
/// the object id of the publication it is for.
pub mod subscription {
    /// The full path of the target database (`ADS_DD_SUBSCR_TARGET`).
    pub const TARGET: usize = 0;
    /// The user name used to connect to the target (`ADS_DD_SUBSCR_USERNAME`).
    pub const USER_NAME: usize = 1;
    /// The target password, enciphered with the dictionary key
    /// (`ADS_DD_SUBSCR_PASSWORD`).
    pub const PASSWORD: usize = 2;
    /// u16: 1 if updates from a replication are forwarded
    /// (`ADS_DD_SUBSCR_FORWARD`).
    pub const FORWARD: usize = 3;
    /// u16: 1 if replication is enabled (`ADS_DD_SUBSCR_ENABLED`).
    pub const ENABLED: usize = 4;
    /// The replication queue, relative to the dictionary's folder
    /// (`ADS_DD_SUBSCR_QUEUE_NAME_RELATIVE`; `ADS_DD_SUBSCR_QUEUE_NAME` is this
    /// made absolute).
    pub const QUEUE_NAME: usize = 5;
    /// u32: the subscription options and the communication-type bits
    /// (`ADS_DD_SUBSCR_OPTIONS_INTERNAL`; `ADS_DD_SUBSCR_OPTIONS` is this with
    /// the comm bits masked off). See [`comm`].
    pub const OPTIONS: usize = 6;
    /// u16: 1 if replication is paused (`ADS_DD_SUBSCR_PAUSED`).
    pub const PAUSED: usize = 8;
    /// The connection string, enciphered with the dictionary key
    /// (`ADS_DD_SUBSCR_CONNECTION_STR`).
    pub const CONNECTION_STR: usize = 9;
    /// The user-defined property (`ADS_DD_USER_DEFINED_PROP`).
    pub const USER_DEFINED: usize = 14;

    /// The communication-type bits carried in [`OPTIONS`] (verified against the
    /// engine; the values the commented-out `ADS_*_CONNECTION` constants of
    /// `ace.h` name).
    pub mod comm {
        /// UDP/IP (`ADS_DD_SUBSCR_COMM_UDP_IP`).
        pub const UDP_IP: u32 = 0x0000_0020;
        /// IPX (`ADS_DD_SUBSCR_COMM_IPX`).
        pub const IPX: u32 = 0x0000_0040;
        /// TCP/IP (`ADS_DD_SUBSCR_COMM_TCP_IP`).
        pub const TCP_IP: u32 = 0x0000_0080;
        /// TCP/IP v6 (`ADS_DD_SUBSCR_COMM_TCP_IP_V6`).
        pub const TCP_IP_V6: u32 = 0x0000_0100;
        /// TLS (`ADS_DD_SUBSCR_COMM_TLS`).
        pub const TLS: u32 = 0x0000_1000;
        /// Every comm bit: the mask `ADS_DD_SUBSCR_OPTIONS` clears to leave the
        /// plain subscription options.
        pub const MASK: u32 = TCP_IP | TCP_IP_V6 | UDP_IP | IPX | TLS;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::Value as Json;

    use super::*;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
    }

    fn variant(name: &str) -> Dictionary {
        let path = fixtures()
            .join("dictionary/variants")
            .join(format!("{name}.add"));
        Dictionary::open(&path).unwrap_or_else(|e| panic!("{name}: {e:?}"))
    }

    /// The database object of variant `name`.
    fn database_of(name: &str) -> Object {
        variant(name).database().unwrap().clone()
    }

    /// The object of kind `kind` named `name` in variant `variant_name`.
    fn object_of(variant_name: &str, kind: Kind, name: &str) -> Object {
        variant(variant_name).named(kind, name).unwrap().clone()
    }

    #[test]
    fn database_properties_are_where_the_variants_put_them() {
        use database::*;
        let base = database_of("base");
        assert!(base.has(LOGIN_NOT_REQUIRED) && base.has(RIGHTS_NOT_CHECKED));
        assert_eq!((base.u32(VERSION), base.info1), (Some(2), Some(2)));
        assert!(!database_of("db_login_required").has(LOGIN_NOT_REQUIRED));
        assert!(!database_of("db_verify_rights").has(RIGHTS_NOT_CHECKED));
        assert_eq!(
            database_of("db_enable_internet").u16(ENABLE_INTERNET),
            Some(1)
        );
        assert!(database_of("db_encrypt_communication").has(ENCRYPT_COMMUNICATION));
        assert!(database_of("db_disable_dll_caching").has(DISABLE_DLL_CACHING));
        assert_eq!(
            database_of("db_enforce_max_failed").u16(ENFORCE_MAX_FAILED_LOGINS),
            Some(1)
        );
        assert_eq!(
            database_of("db_logins_disabled").u16(LOGINS_DISABLED),
            Some(1)
        );
        assert_eq!(
            database_of("db_security_level").u16(INTERNET_SECURITY_LEVEL),
            Some(1)
        );
        assert_eq!(
            database_of("db_max_failed").u16(MAX_FAILED_ATTEMPTS),
            Some(7)
        );
        assert_eq!(
            database_of("db_query_via_root").u32(QUERY_VIA_ROOT),
            Some(2)
        );
        assert_eq!(
            database_of("db_user_prop").text(USER_DEFINED).as_deref(),
            Some("db prop")
        );
        assert_eq!(
            database_of("db_query_log").text(QUERY_LOG_TABLE).as_deref(),
            Some("qlog")
        );
        assert_eq!(
            database_of("db_default_path")
                .text(DEFAULT_TABLE_PATH)
                .as_deref(),
            Some("..")
        );
        assert_eq!(
            database_of("db_temp_path").text(TEMP_TABLE_PATH).as_deref(),
            Some("..\\base")
        );
        for unset in [
            ENABLE_INTERNET,
            INTERNET_SECURITY_LEVEL,
            MAX_FAILED_ATTEMPTS,
            LOGINS_DISABLED,
        ] {
            assert!(!base.has(unset), "{unset}");
        }
    }

    #[test]
    fn table_properties_are_where_the_variants_put_them() {
        use table::{flags::*, *};
        let flags_of = |name: &str| object_of(name, Kind::Table, "t").info2;
        assert_eq!(flags_of("base"), Some(0));
        assert_eq!(flags_of("table_permission_1"), Some(PERMISSION_LEVEL_1));
        assert_eq!(flags_of("table_permission_3"), Some(PERMISSION_LEVEL_3));
        assert_eq!(flags_of("table_trans_free"), Some(TRANSACTION_FREE));
        assert_eq!(flags_of("table_caching_1"), Some(1 << CACHING_SHIFT));
        assert_eq!(flags_of("table_caching_2"), Some(2 << CACHING_SHIFT));
        let r = |name: &str| object_of(name, Kind::Table, "r").info2;
        assert_eq!(r("table_rowversion"), Some(0));
        assert_eq!(r("table_concurrency"), Some(CONCURRENCY));
        assert_eq!(r("table_web_delta"), Some(WEB_DELTA));
        let base = object_of("base", Kind::Table, "t");
        assert_eq!(base.text(RELATIVE_PATH).as_deref(), Some(".\\t.adt"));
        assert_eq!(base.u16(FIELD_COUNT), Some(2));
        assert!(base.has(NO_AUTO_CREATE) && !base.has(TRIGGERS_DISABLED));
        assert!(!object_of("table_auto_create", Kind::Table, "t").has(NO_AUTO_CREATE));
        assert!(object_of("table_triggers_disabled", Kind::Table, "t").has(TRIGGERS_DISABLED));
        let prop = object_of("table_user_prop", Kind::Table, "t").text(USER_DEFINED);
        assert_eq!(prop.as_deref(), Some("table prop"));
        let prop = object_of("field_user_prop", Kind::Field, "s").text(field::USER_DEFINED);
        assert_eq!(prop.as_deref(), Some("field prop"));
    }

    #[test]
    fn user_properties_are_where_the_variants_put_them() {
        use user::*;
        let user = object_of("user", Kind::User, "u");
        assert_eq!(user.info1, Some(0));
        assert_eq!(user.u16(ENABLE_INTERNET), None, "set, but empty");
        assert!(user.has(ENABLE_INTERNET) && !user.has(NO_OLD_PASSWORD));
        assert_eq!(object_of("user_in_group", Kind::User, "u").info1, Some(1));
        assert_eq!(
            object_of("user_in_two_groups", Kind::User, "u").info1,
            Some(2)
        );
        assert_eq!(
            object_of("user_internet", Kind::User, "u").u16(ENABLE_INTERNET),
            Some(1)
        );
        assert_eq!(
            object_of("user_logins_disabled", Kind::User, "u").u16(LOGINS_DISABLED),
            Some(1)
        );
        assert!(object_of("user_no_old_password", Kind::User, "u").has(NO_OLD_PASSWORD));
        let prop = object_of("user_prop", Kind::User, "u").text(USER_DEFINED);
        assert_eq!(prop.as_deref(), Some("user prop"));
        assert_eq!(variant("group").all(Kind::Group).len(), 1);
    }

    /// Checks every kind of secret of an AES dictionary made by the engine
    /// (`tools/ace/crypto/aesdict.sh`): the ADSSYS and `u1` (`userpw`) logins
    /// and the key they unwrap, `u1`'s membership of `g1`, and the rights of
    /// the grants on `t`, whose CRCs check only under the right key.
    fn check_aes_secrets(dict: &Dictionary, scheme: crate::crypto::Scheme, admin: &[u8]) {
        use crate::crypto;
        assert_eq!(dict.scheme(), scheme);
        let key = dict
            .dictionary_key(admin)
            .expect("the ADSSYS password unwraps the key");
        assert_eq!(dict.dictionary_key(b"wrong"), None);
        let u1 = dict.named(Kind::User, "u1").unwrap();
        let secret = u1.slot(user::PASSWORD_SECRET).unwrap();
        assert!(crypto::user_password_ok(
            scheme,
            secret,
            u1.record(),
            0,
            b"userpw"
        ));
        assert!(!crypto::user_password_ok(
            scheme,
            secret,
            u1.record(),
            0,
            b"userpx"
        ));
        let user_key = crypto::decode_user_secret(scheme, secret, u1.record(), 0, b"userpw");
        assert_eq!(user_key.unwrap().key, key);
        let g1 = dict.named(Kind::Group, "g1").unwrap();
        let mut membership = u1.slot(user::GROUP_MEMBERSHIP).unwrap().to_vec();
        crypto::group_membership(scheme, &key, &mut membership);
        assert_eq!(membership, g1.id.to_le_bytes());
        let grants = dict.all(Kind::Permission);
        assert!(!grants.is_empty());
        for grant in grants {
            let mut blob = grant.slot(permission::OBJECT_RIGHTS).unwrap().to_vec();
            let item = crypto::permission_item(grant.id, permission::OBJECT_RIGHTS as u32);
            crypto::crypt(scheme, &key, item, &mut blob);
            assert!(
                crypto::permission_rights(&blob).is_some(),
                "grant {}",
                grant.id
            );
        }
    }

    /// `fixtures/sql/ddbuiltin`: the groups the original engine left each user
    /// in, as its membership slot holds them.
    #[test]
    fn reads_the_built_in_groups_of_a_user() {
        use crate::crypto;
        let dict = Dictionary::open(&fixtures().join("sql/ddbuiltin/bg.add")).unwrap();
        let key = dict.dictionary_key(b"").unwrap();
        let mut groups = Vec::new();
        for user in dict.all(Kind::User) {
            let mask = user.slot(user::BUILT_IN_MEMBERSHIP).map(|slot| {
                let mut data = slot.to_vec();
                crypto::crypt(
                    dict.scheme(),
                    &key,
                    crypto::built_in_membership_item(user.id),
                    &mut data,
                );
                crypto::built_in_membership_mask(&data).expect("a zero where there is one")
            });
            let names: Vec<&str> = BUILT_IN_GROUPS
                .into_iter()
                .filter(|g| {
                    built_in_membership_bit(g).is_some_and(|bit| mask.unwrap_or(0) & bit != 0)
                })
                .collect();
            groups.push((user.name.clone(), mask.is_some(), names));
        }
        let want: [(&str, bool, &[&str]); 8] = [
            ("u1", true, &["DB:Admin"]),
            ("u2", true, &["DB:Backup"]),
            ("u3", true, &["DB:Debug"]),
            ("u4", true, &["SERVER:Admin"]),
            ("u5", true, &["DB:Admin", "SERVER:Monitor"]),
            ("u6", true, &["DB:Admin", "DB:Backup"]),
            ("u7", true, &["DB:Admin"]),
            ("an3", false, &[]),
        ];
        assert_eq!(groups.len(), want.len());
        for ((name, held, names), (want_name, want_held, want_names)) in groups.iter().zip(want) {
            assert_eq!(
                (name.as_str(), *held, names.as_slice()),
                (want_name, want_held, want_names)
            );
        }
        // A blob written back is read back, whatever its salt; one with a changed
        // byte is not.
        let blob = crypto::built_in_membership_blob(0x2000_0001, [1, 2, 3, 4]);
        assert_eq!(crypto::built_in_membership_mask(&blob), Some(0x2000_0001));
        let mut changed = blob;
        changed[12] ^= 1;
        assert_eq!(crypto::built_in_membership_mask(&changed), None);
    }

    #[test]
    fn reads_the_secrets_of_aes_dictionaries() {
        use crate::crypto::Scheme;
        let dict = |name: &str| fixtures().join(format!("aes/dict/{name}/{name}.add"));
        // Records in the clear: no dictionary password needed.
        let a2 = Dictionary::open(&dict("a2")).unwrap();
        check_aes_secrets(&a2, Scheme::Aes128, b"");
        let a3 = Dictionary::open(&dict("a3")).unwrap();
        check_aes_secrets(&a3, Scheme::Aes128, b"admpw");
        let a4 = Dictionary::open(&dict("a4")).unwrap();
        check_aes_secrets(&a4, Scheme::Aes256, b"admpw");
        // e3 enciphers its records under its dictionary password.
        assert!(matches!(
            Dictionary::open(&dict("e3")),
            Err(Error::Encrypted)
        ));
        assert!(matches!(
            Dictionary::open_with_password(&dict("e3"), Some(b"wrong")),
            Err(Error::WrongPassword)
        ));
        let e3 = Dictionary::open_with_password(&dict("e3"), Some(b"ddpw")).unwrap();
        assert!(e3.encrypted());
        assert!(e3.named(Kind::Table, "t2").is_some());
        check_aes_secrets(&e3, Scheme::Aes128, b"admpw");
        // The older fixtures, encrypted too, with no ADSSYS password.
        let aes256 = fixtures().join("aes/aes256/aes256.add");
        let aes256 = Dictionary::open_with_password(&aes256, Some(b"ddsecret")).unwrap();
        assert_eq!(aes256.scheme(), Scheme::Aes256);
        assert!(aes256.named(Kind::Table, "big").is_some());
        // With no ADSSYS password any secret checks; the length byte of the
        // absent table password is what deciphers only under AES.
        let database = aes256.database().unwrap();
        let mut slot = database.slot(1).unwrap().to_vec();
        crate::crypto::obfuscate(Scheme::Aes256, &mut slot, database.record, 1);
        assert_eq!((slot.len(), slot[0]), (41, 0));
        // An RC4 dictionary keeps RC4.
        let rc4 = Dictionary::open(&fixtures().join("adt/dictionary_encrypted/rc4.add")).unwrap();
        assert_eq!(rc4.scheme(), Scheme::Rc4);
    }

    #[test]
    fn opens_an_rc4_encrypted_dictionary() {
        // An RC4-encrypted dictionary's records are obfuscated with AtGdQ; its
        // objects read once that is undone.
        let path = fixtures().join("adt/dictionary_encrypted/rc4.add");
        let dict = Dictionary::open(&path).unwrap();
        let tables: Vec<&str> = dict
            .all(Kind::Table)
            .iter()
            .map(|o| o.name.as_str())
            .collect();
        assert!(tables.contains(&"rc4_secret"), "tables found: {tables:?}");
        assert!(tables.contains(&"rc4_plain") && tables.contains(&"rc4_memos"));
        // The dictionary's table password is recovered with the admin password.
        assert_eq!(dict.table_password(b"adssyspw"), Some(b"tblpass".to_vec()));
        assert_eq!(dict.table_password(b"wrong"), None);
        // A non-dictionary file still fails.
        let path = fixtures().join("adt/dictionary/customers.adt");
        assert!(matches!(Dictionary::open(&path), Err(Error::Read(_))));
    }

    /// A system table's rows from a `<dict>.system.json`: each column's
    /// text without trailing spaces, `None` for NULL.
    fn rows(system: &Json, table: &str) -> Vec<HashMap<String, Option<String>>> {
        let records = system[table]["records"].as_array().unwrap();
        records
            .iter()
            .map(|record| {
                let values = record["values"].as_object().unwrap();
                values
                    .iter()
                    .map(|(column, value)| {
                        let text = value["text"].as_str().map(|t| t.trim_end().to_owned());
                        (column.clone(), text)
                    })
                    .collect()
            })
            .collect()
    }

    /// `T` or `F`.
    fn logical(value: bool) -> Option<String> {
        Some(if value { "T" } else { "F" }.to_owned())
    }

    fn number(value: Option<impl ToString>) -> Option<String> {
        value.map(|v| v.to_string())
    }

    /// Text as a system table shows it: trailing spaces trimmed.
    fn trimmed(text: Option<String>) -> Option<String> {
        text.map(|t| t.trim_end().to_owned())
    }

    /// Checks `row` against the columns `expected` gives.
    fn check(
        what: &str,
        row: &HashMap<String, Option<String>>,
        expected: &[(&str, Option<String>)],
    ) {
        for (column, value) in expected {
            assert_eq!(&row[*column], value, "{what}: {column}");
        }
    }

    /// The name of the object numbered `id`, if any.
    fn name_of(dd: &Dictionary, id: Option<u32>) -> Option<String> {
        id.and_then(|id| dd.object(id)).map(|o| o.name.clone())
    }

    /// Every dictionary fixture's objects against what its `system.*`
    /// tables show of them.
    #[test]
    fn objects_match_the_system_tables() {
        for (folder, name) in [
            ("dictionary", "dict"),
            ("psm", "psm"),
            ("dictionary_props", "props"),
        ] {
            let dir = fixtures().join("adt").join(folder);
            let dd = Dictionary::open(&dir.join(format!("{name}.add"))).unwrap();
            let json = std::fs::read_to_string(dir.join(format!("{name}.system.json"))).unwrap();
            let system: Json = serde_json::from_str(&json).unwrap();
            check_database(&dd, &rows(&system, "system.dictionary")[0]);
            check_tables(&dd, &system);
            check_code(&dd, &system);
            check_users(&dd, &system);
        }
    }

    fn check_database(dd: &Dictionary, row: &HashMap<String, Option<String>>) {
        use database::*;
        let db = dd.database().unwrap();
        let flag = |slot| logical(db.has(slot));
        let u16_or = |slot, default: u16| number(Some(db.u16(slot).unwrap_or(default)));
        check(
            "database",
            row,
            &[
                ("Version_Major", u16_or(VERSION_MAJOR, 0)),
                ("Version_Minor", u16_or(VERSION_MINOR, 0)),
                ("Log_In_Required", logical(!db.has(LOGIN_NOT_REQUIRED))),
                ("Verify_Access_Rights", logical(!db.has(RIGHTS_NOT_CHECKED))),
                (
                    "Enable_Internet",
                    logical(db.u16(ENABLE_INTERNET) == Some(1)),
                ),
                (
                    "Internet_Security_Level",
                    u16_or(INTERNET_SECURITY_LEVEL, 2),
                ),
                ("Max_Failed_Attempts", u16_or(MAX_FAILED_ATTEMPTS, 5)),
                (
                    "Logins_Disabled",
                    logical(db.u16(LOGINS_DISABLED) == Some(1)),
                ),
                ("Logins_Disabled_Msg", db.text(LOGINS_DISABLED_MESSAGE)),
                ("Comment", db.comment.clone()),
                ("User_Defined_Prop", db.text(USER_DEFINED)),
                ("FTS_Delimiters", db.text(FTS_DELIMITERS)),
                ("FTS_Noise_Words", db.text(FTS_NOISE)),
                ("FTS_Drop_Chars", db.text(FTS_DROP_CHARS)),
                ("FTS_Conditional_Chars", db.text(FTS_CONDITIONAL_CHARS)),
                ("Query_Log_Table", db.text(QUERY_LOG_TABLE)),
                ("Encrypt_Communication", flag(ENCRYPT_COMMUNICATION)),
                ("Disable_DLL_Caching", flag(DISABLE_DLL_CACHING)),
                ("Data_Encryption_Type", number(db.u32(DATA_ENCRYPTION_TYPE))),
                (
                    "Query_Via_Root",
                    number(Some(db.u32(QUERY_VIA_ROOT).unwrap_or(0))),
                ),
                (
                    "Enforce_Max_Failed_Logins",
                    logical(db.u16(ENFORCE_MAX_FAILED_LOGINS) == Some(1)),
                ),
            ],
        );
    }

    fn check_tables(dd: &Dictionary, system: &Json) {
        let tables = rows(system, "system.tables");
        assert_eq!(tables.len(), dd.all(Kind::Table).len());
        for row in &tables {
            let name = row["Name"].clone().unwrap();
            let table = dd.named(Kind::Table, &name).unwrap();
            let flags = table.info2.unwrap_or(0);
            let level = match flags {
                f if f & table::flags::PERMISSION_LEVEL_1 != 0 => 1,
                f if f & table::flags::PERMISSION_LEVEL_3 != 0 => 3,
                _ => 2,
            };
            let primary_key = name_of(dd, table.info1.filter(|&i| i != 0).map(|i| i as u32));
            check(
                &name,
                row,
                &[
                    ("Table_Relative_Path", table.text(table::RELATIVE_PATH)),
                    ("Table_Type", Some("3".to_owned())),
                    (
                        "Table_Auto_Create",
                        logical(!table.has(table::NO_AUTO_CREATE)),
                    ),
                    ("Table_Primary_Key", primary_key),
                    ("Table_Default_Index", table.text(table::DEFAULT_INDEX)),
                    ("Table_Permission_Level", number(Some(level))),
                    (
                        "Table_Memo_Block_Size",
                        number(table.u16(table::MEMO_BLOCK_SIZE)),
                    ),
                    ("Table_Validation_Expr", table.text(table::VALIDATION_EXPR)),
                    ("Table_Validation_Msg", table.text(table::VALIDATION_MSG)),
                    ("Comment", table.comment.clone()),
                    ("User_Defined_Prop", table.text(table::USER_DEFINED)),
                    (
                        "Triggers_Disabled",
                        logical(table.has(table::TRIGGERS_DISABLED)),
                    ),
                    (
                        "Table_Caching",
                        number(Some(flags >> table::flags::CACHING_SHIFT & 3)),
                    ),
                    (
                        "Table_Trans_Free",
                        logical(flags & table::flags::TRANSACTION_FREE != 0),
                    ),
                    (
                        "Table_WEB_delta",
                        logical(flags & table::flags::WEB_DELTA != 0),
                    ),
                    (
                        "Table_Concurrency_Enabled",
                        logical(flags & table::flags::CONCURRENCY != 0),
                    ),
                ],
            );
        }

        let columns = rows(system, "system.columns");
        assert_eq!(columns.len(), dd.all(Kind::Field).len());
        for row in &columns {
            let parent = dd
                .named(Kind::Table, row["Parent"].as_deref().unwrap())
                .unwrap();
            let name = row["Name"].clone().unwrap();
            let field = dd.child(parent.id, Kind::Field, &name).unwrap();
            let definition = field.slot(field::DEFINITION).unwrap();
            let u16_at = |i: usize| u16::from_le_bytes([definition[2 * i], definition[2 * i + 1]]);
            check(
                &name,
                row,
                &[
                    ("Field_Num", number(field.info1)),
                    ("Field_Type", number(Some(u16_at(0)))),
                    ("Field_Length", number(Some(u16_at(1)))),
                    ("Field_Decimal", number(Some(u16_at(2)))),
                    ("Field_Min_Value", field.text(field::MIN_VALUE)),
                    ("Field_Max_Value", field.text(field::MAX_VALUE)),
                    ("Field_Can_Be_Null", logical(!field.has(field::NOT_NULL))),
                    ("Field_Default_Value", field.text(field::DEFAULT_VALUE)),
                    ("Field_Validation_Msg", field.text(field::VALIDATION_MSG)),
                    ("Comment", field.comment.clone()),
                    ("User_Defined_Prop", field.text(field::USER_DEFINED)),
                    ("Field_Options", number(field.u32(field::OPTIONS))),
                ],
            );
        }

        let files = rows(system, "system.indexfiles");
        assert_eq!(files.len(), dd.all(Kind::IndexFile).len());
        for row in &files {
            let parent = dd
                .named(Kind::Table, row["Parent"].as_deref().unwrap())
                .unwrap();
            let name = row["Name"].clone().unwrap();
            let file = dd.child(parent.id, Kind::IndexFile, &name).unwrap();
            check(
                &name,
                row,
                &[
                    ("Index_File_Path", file.text(index_file::RELATIVE_PATH)),
                    (
                        "Index_File_PageSize",
                        number(file.u32(index_file::PAGE_SIZE)),
                    ),
                ],
            );
        }

        let indexes = rows(system, "system.indexes");
        assert_eq!(indexes.len(), dd.all(Kind::Index).len());
        for row in &indexes {
            let parent = dd
                .named(Kind::Table, row["Parent"].as_deref().unwrap())
                .unwrap();
            let name = row["Name"].clone().unwrap();
            let index = dd.child(parent.id, Kind::Index, &name).unwrap();
            let options = index.u32(index::OPTIONS).unwrap();
            let options = options | if options & 1 != 0 { 0x800 } else { 0 };
            check(
                &name,
                row,
                &[
                    (
                        "Index_File_Name",
                        name_of(dd, index.info1.map(|i| i as u32)),
                    ),
                    ("Index_Expression", index.text(index::EXPRESSION)),
                    ("Index_Condition", index.text(index::CONDITION)),
                    ("Index_Options", number(Some(options))),
                    ("Index_Key_Length", number(index.u16(index::KEY_LENGTH))),
                    ("Comment", index.comment.clone()),
                ],
            );
        }

        for row in &rows(system, "system.relations") {
            let name = row["Name"].clone().unwrap();
            let rule = dd.named(Kind::Relation, &name).unwrap();
            check(
                &name,
                row,
                &[
                    (
                        "RI_Primary_Table",
                        name_of(dd, rule.u32(relation::PRIMARY_TABLE)),
                    ),
                    (
                        "RI_Primary_Index",
                        name_of(dd, rule.u32(relation::PRIMARY_INDEX)),
                    ),
                    (
                        "RI_Foreign_Table",
                        name_of(dd, rule.u32(relation::FOREIGN_TABLE)),
                    ),
                    (
                        "RI_Foreign_Index",
                        name_of(dd, rule.u32(relation::FOREIGN_INDEX)),
                    ),
                    ("RI_UpdateRule", number(rule.u8(relation::UPDATE_RULE))),
                    ("RI_DeleteRule", number(rule.u8(relation::DELETE_RULE))),
                    ("RI_No_PKey_Error", rule.text(relation::NO_PKEY_ERROR)),
                    ("RI_Cascade_Error", rule.text(relation::CASCADE_ERROR)),
                ],
            );
        }
    }

    /// Views, procedures, functions, packages and triggers.
    fn check_code(dd: &Dictionary, system: &Json) {
        let views = rows(system, "system.views");
        assert_eq!(views.len(), dd.all(Kind::View).len());
        for row in &views {
            let name = row["Name"].clone().unwrap();
            let view = dd.named(Kind::View, &name).unwrap();
            check(
                &name,
                row,
                &[
                    ("View_Stmt", trimmed(view.text(view::STATEMENT))),
                    ("View_Stmt_Len", number(view.u16(view::STATEMENT_LENGTH))),
                    ("Comment", view.comment.clone()),
                ],
            );
        }

        let procedures = rows(system, "system.storedprocedures");
        assert_eq!(procedures.len(), dd.all(Kind::Procedure).len());
        for row in &procedures {
            let name = row["Name"].clone().unwrap();
            let procedure = dd.named(Kind::Procedure, &name).unwrap();
            check(
                &name,
                row,
                &[
                    ("Proc_Input", procedure.text(procedure::INPUT)),
                    ("Proc_Output", procedure.text(procedure::OUTPUT)),
                    ("Proc_DLL_Name", procedure.text(procedure::DLL_NAME)),
                    (
                        "Proc_DLL_Function_Name",
                        procedure.text(procedure::DLL_FUNCTION_NAME),
                    ),
                    (
                        "Proc_Invoke_Option",
                        number(procedure.u32(procedure::INVOKE_OPTION)),
                    ),
                    ("SQL_Script", trimmed(procedure.text(procedure::SCRIPT))),
                    ("Comment", procedure.comment.clone()),
                ],
            );
        }

        let functions = rows(system, "system.functions");
        assert_eq!(functions.len(), dd.all(Kind::Function).len());
        for row in &functions {
            let name = row["Name"].clone().unwrap();
            let function = dd.named(Kind::Function, &name).unwrap();
            let package = dd
                .object(function.parent)
                .filter(|p| p.kind == Kind::Package);
            check(
                &name,
                row,
                &[
                    ("Package", package.map(|p| p.name.clone())),
                    ("Return Type", function.text(function::RETURN_TYPE)),
                    ("Input Parameters", function.text(function::PARAMETERS)),
                    ("Implementation", trimmed(function.text(function::SCRIPT))),
                    ("Comment", function.comment.clone()),
                ],
            );
        }

        let packages = rows(system, "system.packages");
        assert_eq!(packages.len(), dd.all(Kind::Package).len());
        for row in &packages {
            let name = row["Name"].clone().unwrap();
            let package = dd.named(Kind::Package, &name).unwrap();
            check(&name, row, &[("Comment", package.comment.clone())]);
        }

        let triggers = rows(system, "system.triggers");
        assert_eq!(triggers.len(), dd.all(Kind::Trigger).len());
        for row in &triggers {
            let name = row["Name"].clone().unwrap();
            let trigger = dd.named(Kind::Trigger, &name).unwrap();
            let table = dd
                .object(trigger.info1.unwrap() as u32)
                .filter(|t| t.kind == Kind::Table)
                .map(|t| t.name.clone());
            check(
                &name,
                row,
                &[
                    ("Trig_TableName", table),
                    ("Trig_Event_Type", number(trigger.u32(trigger::EVENT_TYPE))),
                    (
                        "Trig_Trigger_Type",
                        number(trigger.u32(trigger::TRIGGER_TYPE)),
                    ),
                    (
                        "Trig_Container_Type",
                        number(trigger.u32(trigger::CONTAINER_TYPE)),
                    ),
                    ("Trig_Container", trimmed(trigger.text(trigger::CONTAINER))),
                    ("Trig_Function_Name", trigger.text(trigger::FUNCTION_NAME)),
                    ("Trig_Priority", number(trigger.u32(trigger::PRIORITY))),
                    ("Trig_Options", number(trigger.u32(trigger::OPTIONS))),
                    ("Comment", trimmed(trigger.comment.clone())),
                ],
            );
        }
    }

    /// Users, groups and links.
    fn check_users(dd: &Dictionary, system: &Json) {
        let users = rows(system, "system.users");
        assert_eq!(users.len(), dd.all(Kind::User).len());
        for row in &users {
            let name = row["Name"].clone().unwrap();
            let user = dd.named(Kind::User, &name).unwrap();
            check(
                &name,
                row,
                &[
                    (
                        "Enable_Internet",
                        logical(user.u16(user::ENABLE_INTERNET) == Some(1)),
                    ),
                    (
                        "Logins_Disabled",
                        logical(user.u16(user::LOGINS_DISABLED) == Some(1)),
                    ),
                    (
                        "Require_Old_Password",
                        logical(!user.has(user::NO_OLD_PASSWORD)),
                    ),
                    ("User_Defined_Prop", user.text(user::USER_DEFINED)),
                    ("Comment", user.comment.clone()),
                ],
            );
        }

        let groups: Vec<_> = rows(system, "system.usergroups");
        let names: Vec<_> = groups.iter().map(|g| g["Name"].clone().unwrap()).collect();
        let mut expected: Vec<_> = dd.all(Kind::Group).iter().map(|g| g.name.clone()).collect();
        expected.extend(BUILT_IN_GROUPS.iter().map(|g| g.to_string()));
        assert_eq!(names, expected);
        for row in &groups {
            let name = row["Name"].clone().unwrap();
            let comment = dd.named(Kind::Group, &name).and_then(|g| g.comment.clone());
            check(&name, row, &[("Comment", comment)]);
        }

        for row in &rows(system, "system.links") {
            let name = row["Name"].clone().unwrap();
            let link = dd.named(Kind::Link, &name).unwrap();
            check(
                &name,
                row,
                &[
                    ("Link_Path", link.text(link::RELATIVE_PATH)),
                    ("Link_Options", number(link.info1)),
                    ("Link_UserName", link.text(link::USER_NAME)),
                ],
            );
        }
    }

    /// The property-field ranges of the Database object's three encrypted
    /// login slots (0, 1 and 4), which the engine fills with random salts and
    /// so differ every time a dictionary is made. Everything else is fixed.
    fn login_slot_ranges(add: &[u8]) -> Vec<std::ops::Range<usize>> {
        let data_offset = u32::from_le_bytes(add[0x20..0x24].try_into().unwrap()) as usize;
        let aes = u32::from_le_bytes(add[0x170..0x174].try_into().unwrap()) != 0;
        let property = data_offset + 223 + if aes { 8 } else { 0 };
        let mut ranges = Vec::new();
        let (mut at, mut index) = (0usize, 0usize);
        while at + 2 <= 275 {
            let n = u16::from_le_bytes(add[property + at..property + at + 2].try_into().unwrap());
            if n == 0xFFFF {
                at += 2;
                index += 1;
                continue;
            }
            if [0, 1, 4].contains(&index) {
                ranges.push(property + at + 2..property + at + 2 + n as usize);
            }
            at += 2 + n as usize;
            index += 1;
        }
        ranges
    }

    #[test]
    fn a_new_aes_dictionary_matches_the_engine() {
        use super::create::{NewAes, NewDictRandom, new_aes_dictionary_add};
        let random = NewDictRandom {
            secret_first: [0x5a; 20],
            secret_last: [0xa5; 20],
            slot0: [0x11; 20],
            slot4: [0x22; 20],
        };
        let snap = fixtures().join("aes/dictwrite/snap/00_create");
        for name in ["c", "e", "f"] {
            let golden = std::fs::read(snap.join(format!("{name}/{name}.add"))).unwrap();
            // The original's random material, so only the login secrets and
            // the record's IV are left to differ.
            let descriptor_iv: [u8; 8] = golden[0x1C0..0x1C8].try_into().unwrap();
            let aes = NewAes {
                material: crate::create::AesMaterial {
                    enc_type: golden[0x190],
                    salt: golden[0x1A0..0x1C0].try_into().unwrap(),
                    nonce: golden[0x194..0x198].try_into().unwrap(),
                    descriptor_iv,
                    iv_counter: u64::from_le_bytes(descriptor_iv) + 1,
                },
                password: b"ddpw",
                encrypt: golden[0x58] & 0x04 != 0,
            };
            let ours = new_aes_dictionary_add(&random, &aes).unwrap();
            let (mut ours, mut golden) = (deciphered(&ours, b"ddpw"), deciphered(&golden, b"ddpw"));
            for range in login_slot_ranges(&golden) {
                golden[range.clone()].fill(0);
                ours[range].fill(0);
            }
            let first = ours.iter().zip(&golden).position(|(a, b)| a != b);
            assert_eq!((first, ours.len()), (None, golden.len()), "{name}");
            assert_eq!(
                std::fs::read(snap.join(format!("{name}/{name}.am"))).unwrap(),
                super::create::new_dictionary_memo(),
                "{name}.am"
            );
        }
    }

    #[test]
    fn a_new_dictionary_matches_the_engine() {
        use super::create::{NewDictRandom, new_dictionary_add, new_dictionary_memo};
        let golden_add = std::fs::read("../../fixtures/dictionary/new/empty.add").unwrap();
        let golden_am = std::fs::read("../../fixtures/dictionary/new/empty.am").unwrap();

        let random = NewDictRandom {
            secret_first: [0x5a; 20],
            secret_last: [0xa5; 20],
            slot0: [0x11; 20],
            slot4: [0x22; 20],
        };
        let mut ours = new_dictionary_add(&random).unwrap();
        assert_eq!(ours.len(), golden_add.len(), "add length");

        // Mask the three random login slots in both, then the rest must match
        // the engine's dictionary byte for byte.
        let mut golden = golden_add.clone();
        for range in login_slot_ranges(&golden_add) {
            golden[range.clone()].fill(0);
            ours[range].fill(0);
        }
        assert_eq!(ours, golden, ".add differs outside the login slots");
        assert_eq!(new_dictionary_memo(), golden_am, ".am differs");

        // The secret we generated must still authenticate ADSSYS: its slot 1,
        // de-obfuscated, verifies the empty password.
        let data_offset =
            u32::from_le_bytes(ours.as_slice()[0x20..0x24].try_into().unwrap()) as usize;
        let slots = super::slots(
            &new_dictionary_add(&random).unwrap()[data_offset + 223..data_offset + 223 + 275],
        );
        let mut slot1 = slots[1].clone().unwrap();
        crate::crypto::obfuscate(crate::crypto::Scheme::Rc4, &mut slot1, 1, 1);
        assert_eq!(slot1[0], 0, "no wrapped table password");
        assert!(
            crate::crypto::decode_secret(crate::crypto::Scheme::Rc4, &slot1[1..], b"")
                .unwrap()
                .password_ok
        );
    }

    #[test]
    fn decoding_then_re_encoding_a_blob_is_the_identity() {
        use super::create::{PROPERTY_LENGTH, decode_property, set_property_slot};
        // A field blob as the engine writes one (slots 0, 2, 3, 4, 5, 6 set).
        let mut blob = vec![0xFFu8; PROPERTY_LENGTH];
        // Rebuild it by setting each slot, then a round-trip must not change it.
        set_property_slot(&mut blob, 0, b"xx\0").unwrap();
        set_property_slot(&mut blob, 2, b"aa\0").unwrap();
        set_property_slot(&mut blob, 3, b"zz\0").unwrap();
        set_property_slot(&mut blob, 4, b"bad\0").unwrap();
        set_property_slot(&mut blob, 5, &[0x04, 0, 0x0a, 0, 0, 0]).unwrap();
        set_property_slot(&mut blob, 6, &[0, 0, 0, 0]).unwrap();

        let decoded = decode_property(&blob);
        assert_eq!(decoded[0].as_deref(), Some(&b"xx\0"[..]));
        assert_eq!(decoded[1], None, "slot 1 stays unset");
        assert_eq!(decoded[4].as_deref(), Some(&b"bad\0"[..]));
        assert_eq!(decoded.len(), 7, "trailing unset slots are dropped");

        // Re-encoding the decoded slots yields the same bytes.
        let mut again = vec![0xFFu8; PROPERTY_LENGTH];
        for (i, slot) in decoded.iter().enumerate() {
            if let Some(value) = slot {
                set_property_slot(&mut again, i, value).unwrap();
            }
        }
        assert_eq!(again, blob);

        // Replacing a set slot keeps the others and the fill.
        let mut edited = blob.clone();
        set_property_slot(&mut edited, 0, b"yyy\0").unwrap();
        let after = decode_property(&edited);
        assert_eq!(after[0].as_deref(), Some(&b"yyy\0"[..]));
        assert_eq!(after[4].as_deref(), Some(&b"bad\0"[..]));

        // An overlong value is an error, not a panic, and leaves the blob be.
        let mut small = vec![0xFFu8; 8];
        assert!(set_property_slot(&mut small, 0, &[0u8; 20]).is_err());
    }

    #[test]
    fn setting_and_unsetting_a_presence_slot() {
        use super::create::{
            PROPERTY_LENGTH, decode_property, set_property_slot, unset_property_slot,
        };
        let mut blob = vec![0xFFu8; PROPERTY_LENGTH];
        set_property_slot(&mut blob, 5, b"keep\0").unwrap();

        // Setting a presence slot empty makes it present with no data; the
        // middle slots become explicit unset slots (0xFFFF).
        set_property_slot(&mut blob, 1, &[]).unwrap();
        let decoded = decode_property(&blob);
        assert_eq!(
            decoded[1].as_deref(),
            Some(&b""[..]),
            "slot 1 present, empty"
        );
        assert_eq!(decoded[5].as_deref(), Some(&b"keep\0"[..]));

        // Unsetting it again drops it; slot 5 and its data are untouched.
        unset_property_slot(&mut blob, 1).unwrap();
        let decoded = decode_property(&blob);
        assert_eq!(decoded[1], None, "slot 1 unset");
        assert_eq!(decoded[5].as_deref(), Some(&b"keep\0"[..]));

        // Unsetting a slot already unset (even past the end) is a no-op.
        let before = blob.clone();
        unset_property_slot(&mut blob, 1).unwrap();
        unset_property_slot(&mut blob, 99).unwrap();
        assert_eq!(blob, before);
    }
}
