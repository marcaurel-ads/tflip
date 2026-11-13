//! Data dictionaries, as a connection to one sees them: who may connect,
//! the tables and views it opens by name, and its objects and their
//! properties (`AdsDDFindFirstObject`, `AdsDDGet*Property`).
//!
//! What is stored where is [`tflip_storage::dd`]'s; what the calls answer
//! here follows what the original engine answers (`fixtures/dictionary/dd.json`).

use std::path::{MAIN_SEPARATOR, Path, PathBuf};

use serde::{Deserialize, Serialize};
use tflip_storage::crypto;
use tflip_storage::dd::{self, Dictionary, Kind, Object};

use crate::Error;
use crate::table::TableType;

/// The kinds of objects `AdsDDFindFirstObject` lists (the
/// `ADS_DD_*_OBJECT` numbers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectType {
    Table,
    Relation,
    IndexFile,
    Field,
    Index,
    View,
    /// Tables and views (`ADS_DD_VIEW_OR_TABLE_OBJECT`).
    ViewOrTable,
    User,
    Group,
    Procedure,
    Database,
    Link,
    /// Tables, views and the tables of linked dictionaries.
    TableViewOrLink,
    Trigger,
    Publication,
    Article,
    Subscription,
    Function,
    Package,
    /// Triggers named `table::trigger` (a database trigger by its name).
    QualifiedTrigger,
    Permission,
    DatabaseTrigger,
}

impl ObjectType {
    /// The type numbered `number` (`ADS_DD_TABLE_OBJECT`, ...).
    pub fn from_number(number: u16) -> Option<ObjectType> {
        use ObjectType::*;
        Some(match number {
            1 => Table,
            2 => Relation,
            3 => IndexFile,
            4 => Field,
            5 => Index,
            6 => View,
            7 => ViewOrTable,
            8 => User,
            9 => Group,
            10 => Procedure,
            11 => Database,
            12 => Link,
            13 => TableViewOrLink,
            14 => Trigger,
            15 => Publication,
            16 => Article,
            17 => Subscription,
            18 => Function,
            19 => Package,
            20 => QualifiedTrigger,
            21 => Permission,
            22 => DatabaseTrigger,
            _ => return None,
        })
    }
}

/// The object whose property is asked for, by the names the getter takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectRef {
    Database,
    Table(String),
    /// A table's field: the table and the field.
    Field(String, String),
    /// A table's index file: the table and the file.
    IndexFile(String, String),
    /// A table's index: the table and the index.
    Index(String, String),
    Relation(String),
    Group(String),
    View(String),
    Procedure(String),
    User(String),
    Link(String),
    /// A trigger by its qualified name (`table::trigger`).
    Trigger(String),
    /// A replication publication.
    Publication(String),
    /// A replication article: its publication and the published table.
    Article(String, String),
    /// A replication subscription.
    Subscription(String),
}

/// A property's value, as the original engine writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Property {
    /// Text, written with a NUL. Text the dictionary keeps with a NUL of
    /// its own (UTF-8 text, and comments set after their object was made)
    /// ends in it here: the original engine asks a buffer too small for one
    /// more byte then.
    Text(String),
    /// Text written as UTF-16, with a NUL.
    Wide(String),
    U8(u8),
    U16(u16),
    U32(u32),
    /// Bytes written as they are.
    Bytes(Vec<u8>),
    /// A number of this many bytes that is not set.
    Unset(u8),
}

/// What a user may do with a table's columns (see
/// [`Database::column_access`]).
#[derive(Debug, Clone)]
pub(crate) struct ColumnAccess {
    /// The table's name in the dictionary.
    pub table: String,
    /// Its `TABLE_PERMISSION_LEVEL`: 1 lets conditions name the columns the
    /// user may not read, 2 does not, 3 lets only SQL reach the table and
    /// refuses a select list that names one.
    pub level: u8,
    /// Whether a group the user is in holds rights on the table as a whole,
    /// which leaves it every column to read and to update, though a condition
    /// still names only those it holds a right on.
    pub lenient: bool,
    /// Its columns, in order.
    pub columns: Vec<ColumnRights>,
}

/// A table's `TABLE_PERMISSION_LEVEL`.
pub(crate) fn permission_level(table: &Object) -> u8 {
    match table.info2.unwrap_or(0) {
        f if f & dd::table::flags::PERMISSION_LEVEL_1 != 0 => 1,
        f if f & dd::table::flags::PERMISSION_LEVEL_3 != 0 => 3,
        _ => 2,
    }
}

/// What a user may do with one column.
#[derive(Debug, Clone)]
pub(crate) struct ColumnRights {
    pub name: String,
    pub read: bool,
    pub update: bool,
}

impl ColumnAccess {
    /// The names of the columns the user may not update, where it holds the
    /// right on some columns alone.
    pub fn not_updatable(&self) -> Vec<String> {
        match !self.lenient && self.columns.iter().any(|c| c.update) {
            true => (self.columns.iter().filter(|c| !c.update))
                .map(|c| c.name.clone())
                .collect(),
            false => Vec::new(),
        }
    }

    /// The names of the columns the user may not read.
    pub fn hidden(&self) -> Vec<String> {
        self.columns
            .iter()
            .filter(|c| !c.read && !self.lenient)
            .map(|c| c.name.clone())
            .collect()
    }

    /// The column called `name`, whatever its case.
    pub fn column(&self, name: &str) -> Option<&ColumnRights> {
        self.columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }
}

/// A data dictionary a connection is to.
#[derive(Debug)]
pub struct Database {
    dd: Dictionary,
    /// The dictionary's folder, which its relative paths start from.
    folder: PathBuf,
    /// The administrator password the connection supplied, which recovers the
    /// dictionary key that decrypts memberships and permission grants. Empty
    /// for a connection that gave none (the default a dictionary tflip writes),
    /// and only the administrator's own password unlocks the key.
    password: Vec<u8>,
    /// The name of the user the connection logged in as, whatever its case, or
    /// `None` for an anonymous connection. Enforcement checks this user's rights.
    user: Option<String>,
    /// The dictionary password (`DDPassword`) the dictionary was opened with,
    /// which reopening one encrypted with AES needs again.
    dd_password: Option<Vec<u8>>,
    /// The dictionary key `K`, recovered from the logged-in user's own login
    /// secret (every user's secret yields the same `K`). Set at login; it lets
    /// grants and memberships be read for any authenticated user, not only the
    /// administrator.
    user_key: Option<Vec<u8>>,
    /// The data encryption type the dictionary had when the connection opened
    /// it, which `sp_GetSecurityInfo` keeps giving after a
    /// `sp_ModifyDatabase` of the type (`fixtures/aes/convert`, `m_*`; the
    /// connections after it see the new one).
    opened_type: Option<u32>,
}

/// A table of a dictionary, as it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bound {
    /// The table's file.
    pub path: PathBuf,
    /// The table's type.
    pub table_type: crate::table::TableType,
    /// The table's character type, whatever it is opened as: `ADS_OEM` (2)
    /// or `ADS_ANSI` (1).
    pub char_type: u16,
    /// The table's index files, which open with it.
    pub index_files: Vec<PathBuf>,
    /// The name of its primary key's index, if it has one.
    pub primary_key: Option<String>,
    /// The table's object id in the dictionary.
    pub object_id: u32,
}

/// The type of the table `object`, which a dictionary keeps in its `Info1`
/// (probed, `fixtures/dictionary/dbftables`): see [`table_type_info`].
pub(crate) fn table_type(object: &Object) -> TableType {
    match object.info1.unwrap_or(0) {
        -1 => TableType::Cdx,
        -2 => TableType::Ntx,
        -3 => TableType::Vfp,
        _ => TableType::Adt,
    }
}

/// The `ADS_*` number of the type of the table `object`: 1 for `ADS_NTX`, 2
/// for `ADS_CDX`, 3 for `ADS_ADT` and 4 for `ADS_VFP`.
pub(crate) fn ads_table_type(object: &Object) -> u16 {
    match table_type(object) {
        TableType::Ntx => 1,
        TableType::Cdx => 2,
        TableType::Vfp => 4,
        _ => 3,
    }
}

/// The character type of the table `object`, which a dictionary keeps in a
/// bit of its `Info2` (probed, `fixtures/dictionary/dbfcorners`): 2 for
/// `ADS_OEM`, else 1.
pub(crate) fn char_type(object: &Object) -> u16 {
    let oem = object.info2.unwrap_or(0) & dd::table::flags::OEM != 0;
    match table_type(object) {
        TableType::Adt | TableType::Database => 1,
        _ if oem => 2,
        _ => 1,
    }
}

/// The `Info2` a dictionary keeps a table of `table_type` with, of the
/// character type `char_type`: its [`dd::table::flags::OEM`] bit if it is a
/// DBF table of `ADS_OEM` (2).
pub(crate) fn table_info2(table_type: TableType, char_type: u16) -> u32 {
    match table_type {
        TableType::Adt | TableType::Database => 0,
        _ if char_type == 2 => dd::table::flags::OEM as u32,
        _ => 0,
    }
}

/// The `Info1` a dictionary keeps a table of `table_type` with: 0 for an ADT
/// table, -1 for a FoxPro (CDX) one, -2 for a Clipper (NTX) one and -3 for a
/// Visual FoxPro one.
pub(crate) fn table_type_info(table_type: TableType) -> i32 {
    match table_type {
        TableType::Cdx => -1,
        TableType::Ntx => -2,
        TableType::Vfp => -3,
        _ => 0,
    }
}

/// The user that is every dictionary's administrator.
const ADMINISTRATOR: &str = "ADSSYS";

/// Why [`Database::execute_refusal`] refuses an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExecuteRefusal {
    /// Its `DB:Public` slot does not check.
    BadSlot,
    /// The user does not hold the right to execute it.
    NotHeld,
}

impl Database {
    /// The dictionary behind this database.
    pub(crate) fn dictionary(&self) -> &Dictionary {
        &self.dd
    }

    /// The cipher of the dictionary's encrypted slots.
    pub(crate) fn scheme(&self) -> crypto::Scheme {
        self.dd.scheme()
    }

    /// The dictionary password the dictionary was opened with.
    pub(crate) fn dd_password(&self) -> Option<&[u8]> {
        self.dd_password.as_deref()
    }

    /// Reads the dictionary at `path`.
    pub(crate) fn open(path: &Path) -> Result<Database, Error> {
        Database::open_with_password(path, None)
    }

    /// Reads the dictionary at `path`, one encrypted with AES with its
    /// dictionary password `dd_password` (the connect string's `DDPassword`):
    /// without it such a dictionary is refused with 7068, with a wrong one
    /// with 7060, as the original refuses it (`fixtures/aes/dict/aesdict3.json`).
    pub(crate) fn open_with_password(
        path: &Path,
        dd_password: Option<&[u8]>,
    ) -> Result<Database, Error> {
        let dd =
            Dictionary::open_with_password(path, dd_password).map_err(|error| match error {
                dd::Error::Encrypted => Error::DictionaryEncrypted,
                dd::Error::WrongPassword => Error::InvalidPassword,
                dd::Error::Read(_) => Error::DictionaryUnreadable,
            })?;
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let opened_type = dd
            .database()
            .and_then(|database| database.u32(dd::database::DATA_ENCRYPTION_TYPE));
        Ok(Database {
            dd,
            folder,
            password: Vec::new(),
            user: None,
            dd_password: dd_password.map(<[u8]>::to_vec),
            user_key: None,
            opened_type,
        })
    }

    /// The data encryption type the dictionary had when this connection
    /// opened it.
    pub(crate) fn opened_type(&self) -> Option<u32> {
        self.opened_type
    }

    /// Keeps the type an earlier reading of the dictionary opened with.
    pub(crate) fn keep_opened_type(&mut self, opened: Option<u32>) {
        self.opened_type = opened;
    }

    /// Records the administrator `password` the connection gave, used to
    /// recover the dictionary key (see [`Database::key`]).
    pub(crate) fn set_password(&mut self, password: &[u8]) {
        // The engine reads no more than 20 bytes of a password to log in
        // (`fixtures/sql/spddusers.json`).
        self.password = password[..password.len().min(20)].to_vec();
    }

    /// The password the connection gave, kept to re-authenticate and recover
    /// the key when the dictionary is reloaded after a write.
    pub(crate) fn password(&self) -> &[u8] {
        &self.password
    }

    /// The dictionary key `K`: the one recovered from the logged-in user's own
    /// login secret, else — before a login, or for a connection that gave no
    /// user — recovered from the Database object's secret with the supplied
    /// (administrator) password. `None` when nothing unlocks it.
    pub(crate) fn key(&self) -> Option<Vec<u8>> {
        self.user_key
            .clone()
            .or_else(|| self.dd.dictionary_key(&self.password))
    }

    /// Records the identity a login established: the user name and the
    /// dictionary key recovered from its login secret (see [`Database::login`]).
    pub(crate) fn set_identity(&mut self, user: Option<&str>, key: Option<Vec<u8>>) {
        self.user = user.filter(|u| !u.is_empty()).map(str::to_owned);
        self.user_key = key;
    }

    /// Checks that `user` may connect with `password`, as the original engine
    /// does (probed, `tools/ace/dd/login.c`). The user name is matched
    /// case-insensitively, the password exactly and case-sensitively; login
    /// fails (7078) for an unknown user or a wrong password. `ADSSYS` is
    /// authenticated by the administrator password that recovers the dictionary
    /// key; every other user by the login secret in its `PASSWORD_SECRET` slot.
    /// With no user name, no password is checked: the connection is anonymous
    /// and succeeds only where the dictionary does not require logins.
    ///
    /// Returns the dictionary key `K` recovered from the login (for `ADSSYS`
    /// from the admin password, for a user from its own secret — both yield the
    /// same `K`), or `None` for an anonymous connection, to be kept for reading
    /// grants and memberships (see [`Database::set_identity`]).
    pub(crate) fn login(
        &self,
        user: Option<&str>,
        password: &[u8],
    ) -> Result<Option<Vec<u8>>, Error> {
        let password = &password[..password.len().min(20)];
        let database = self.dd.database().ok_or(Error::DictionaryUnreadable)?;
        match user.filter(|u| !u.is_empty()) {
            None if database.has(dd::database::LOGIN_NOT_REQUIRED) => Ok(None),
            None => Err(Error::LoginFailed),
            Some(user) if user.eq_ignore_ascii_case(ADMINISTRATOR) => self
                .dd
                .dictionary_key(password)
                .map(Some)
                .ok_or(Error::LoginFailed),
            Some(user) => {
                let object = self.dd.named(Kind::User, user).ok_or(Error::LoginFailed)?;
                let secret = object
                    .slot(dd::user::PASSWORD_SECRET)
                    .ok_or(Error::LoginFailed)?;
                match crypto::decode_user_secret(
                    self.scheme(),
                    secret,
                    object.record(),
                    0,
                    password,
                ) {
                    Some(decoded) if decoded.password_ok && secret.len() == 40 + password.len() => {
                        Ok(Some(decoded.key))
                    }
                    _ => Err(Error::LoginFailed),
                }
            }
        }
    }

    /// The refusal of a user whose own logins are disabled (7113), if
    /// `user` is one.
    pub(crate) fn user_disabled(&self, user: &str) -> Option<Error> {
        self.dd
            .named(Kind::User, user)
            .filter(|u| u.u16(dd::user::LOGINS_DISABLED) == Some(1))
            .map(|_| Self::logins_disabled_for_user())
    }

    /// 7113.
    pub(crate) fn logins_disabled_for_user() -> Error {
        Error::Constraint {
            code: 7113,
            text: "Database logins for this user have been disabled.  axServerConnect".to_owned(),
        }
    }

    /// How many failed logins lock a user out, if the dictionary has the
    /// failures counted for every remote connection
    /// (`ENFORCE_MAX_FAILED_LOGINS`): its `MAX_FAILED_ATTEMPTS`, 5 if that
    /// was never set.
    pub(crate) fn failed_login_limit(&self) -> Option<u16> {
        let database = self.dd.database()?;
        (database.u16(dd::database::ENFORCE_MAX_FAILED_LOGINS) == Some(1))
            .then(|| database.u16(dd::database::MAX_FAILED_ATTEMPTS).unwrap_or(5))
    }

    /// The record of the dictionary's own user `user`, as the file numbers
    /// it; `None` for `ADSSYS` and unknown names.
    pub(crate) fn user_record(&self, user: &str) -> Option<u32> {
        self.dd.named(Kind::User, user).map(|u| u.record())
    }

    /// Whether `user` names the administrator, whatever the case.
    pub(crate) fn is_administrator_name(user: &str) -> bool {
        user.eq_ignore_ascii_case(ADMINISTRATOR)
    }

    /// Whether a login that [`Database::login`] accepted may go on: the
    /// administrator's always may, anyone else's not while the database's
    /// logins are disabled (7109, with the message the database gives) or the
    /// user's own are (7113) (`fixtures/sql/ddlogins`).
    pub(crate) fn check_logins_enabled(&self, user: Option<&str>) -> Result<(), Error> {
        let Some(user) = user.filter(|u| !u.is_empty() && !u.eq_ignore_ascii_case(ADMINISTRATOR))
        else {
            return Ok(());
        };
        let database = self.dd.database().ok_or(Error::DictionaryUnreadable)?;
        if database.u16(dd::database::LOGINS_DISABLED) == Some(1) {
            let mut text = String::from(
                "Database logins are currently disabled or are prevented by a connection trigger.",
            );
            if let Some(message) = database
                .text(dd::database::LOGINS_DISABLED_MESSAGE)
                .filter(|m| !m.is_empty())
            {
                text.push_str("  ");
                text.push_str(&message);
            }
            return Err(Error::Constraint { code: 7109, text });
        }
        match self.user_disabled(user) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Whether this connection enforces the dictionary's access rights on the
    /// user: the dictionary verifies access rights (its `VERIFY_ACCESS_RIGHTS`
    /// is on), a non-administrator user is logged in, and the administrator does
    /// not bypass (see [`Database::is_admin`]). The original engine checks
    /// rights only when this is so (probed, `tools/ace/dd/rights.c`).
    pub(crate) fn enforces(&self) -> bool {
        let verified = self
            .dd
            .database()
            .is_some_and(|db| !db.has(dd::database::RIGHTS_NOT_CHECKED));
        verified && self.user.is_some() && !self.is_admin()
    }

    /// Whether the dictionary verifies access rights and a user is logged
    /// in, the administrator too: what reading an object's `DB:Public` slot
    /// before running it depends on (`fixtures/sql/ddpublicbad4`).
    pub(crate) fn verifies_rights(&self) -> bool {
        self.user.is_some()
            && self
                .dd
                .database()
                .is_some_and(|db| !db.has(dd::database::RIGHTS_NOT_CHECKED))
    }

    /// Whether the connection is a user other than the administrator and the
    /// members of `DB:Admin`, which the listings that read a `DB:Public` slot
    /// fail for whether or not the dictionary verifies rights
    /// (`fixtures/sql/ddpublicbad4`, `ddpublicbad6`).
    pub(crate) fn is_ordinary_user(&self) -> bool {
        self.user.is_some() && !self.is_admin() && !self.is_admin_member()
    }

    /// Why the connection may not execute the procedure, function or package
    /// `object`, if it may not (`fixtures/sql/ddpublicbad3`, `ddexecrights`):
    /// a `DB:Public` slot that does not check, for anyone the dictionary
    /// verifies; a right not held, for a user held to its rights.
    pub(crate) fn execute_refusal(&self, object: &Object) -> Option<ExecuteRefusal> {
        use crate::system::{permissions_mask, right};
        if !self.verifies_rights() {
            return None;
        }
        if self.public_slot_bad(object) {
            return Some(ExecuteRefusal::BadSlot);
        }
        let object_type = match object.kind {
            Kind::Procedure => 10,
            Kind::Function => 18,
            Kind::Package => 19,
            _ => return None,
        };
        let user = self.user.as_deref().filter(|_| self.enforces())?;
        if self.is_admin_member() {
            return None;
        }
        match permissions_mask(self, user, object_type, &object.name, "", true) {
            Ok(mask) if mask & right::EXECUTE != 0 => None,
            _ => Some(ExecuteRefusal::NotHeld),
        }
    }

    /// The user the connection logged in as, if not anonymously.
    pub(crate) fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Whether `user` may log in at all: the administrator, or a user of the
    /// dictionary, whatever the case.
    pub(crate) fn has_user(&self, user: &str) -> bool {
        user.eq_ignore_ascii_case(ADMINISTRATOR) || self.dd.named(Kind::User, user).is_some()
    }

    /// The dictionary's folder, which its relative paths start from.
    pub(crate) fn folder(&self) -> &Path {
        &self.folder
    }

    /// Whether the connection is the administrator, who bypasses rights checks.
    pub(crate) fn is_admin(&self) -> bool {
        self.user
            .as_deref()
            .is_some_and(|u| u.eq_ignore_ascii_case(ADMINISTRATOR))
    }

    /// The rights the logged-in user holds on the table `name`: the full mask
    /// when rights are not enforced or the user is the administrator, else the
    /// user's effective rights on the table (its own grants OR'd with every
    /// column's, through its groups). A table the user holds no right on yields
    /// an empty mask — it may not even be opened.
    pub(crate) fn access_mask(&self, name: &str) -> u32 {
        if !self.enforces() {
            return u32::MAX;
        }
        match &self.user {
            Some(user) => crate::system::table_access_mask(self, user, name),
            None => 0,
        }
    }

    /// Whether the connection's user is a member of `DB:Admin`, who holds
    /// every right.
    fn in_db_admin(&self, user: &str) -> bool {
        self.dd
            .named(Kind::User, user)
            .is_some_and(|user| self.built_in_groups(user).contains(&"DB:Admin"))
    }

    /// The built-in groups but `DB:Public` that `user` is a member of, in the
    /// order of [`dd::BUILT_IN_GROUPS`]: its built-in membership slot deciphered
    /// with the dictionary key (none where the key cannot be recovered).
    pub(crate) fn built_in_groups(&self, user: &Object) -> Vec<&'static str> {
        let mask = self
            .key()
            .zip(user.slot(dd::user::BUILT_IN_MEMBERSHIP))
            .and_then(|(key, slot)| {
                let mut data = slot.to_vec();
                let item = crypto::built_in_membership_item(user.id);
                crypto::crypt(self.scheme(), &key, item, &mut data);
                crypto::built_in_membership_mask(&data)
            })
            .unwrap_or(0);
        dd::BUILT_IN_GROUPS
            .into_iter()
            .filter(|group| dd::built_in_membership_bit(group).is_some_and(|bit| mask & bit != 0))
            .collect()
    }

    /// Whether the logged-in user holds the right `bit` on the object of
    /// `object_type` named `name`: the administrator always does, and a
    /// connection that gave no user never does. What the objects of a
    /// dictionary may be changed by is checked whether or not it verifies
    /// access rights to the data (`fixtures/sql/ddfuncs`).
    pub(crate) fn may(&self, object_type: u16, name: &str, bit: u32) -> bool {
        if self.is_admin() {
            return true;
        }
        let Some(user) = self.user() else {
            return false;
        };
        self.in_db_admin(user)
            || crate::system::permissions_mask(self, user, object_type, name, "", true)
                .is_ok_and(|mask| mask & bit != 0)
    }

    /// Whether the logged-in user may create objects of the type
    /// `object_type`: as [`Database::may`].
    pub(crate) fn may_create_type(&self, object_type: u16) -> bool {
        if self.is_admin() {
            return true;
        }
        let Some(user) = self.user() else {
            return false;
        };
        self.in_db_admin(user)
            || crate::system::holds_create_right(self, user, i64::from(object_type))
    }

    /// Whether the connection's user is held to the grant option in what it
    /// grants and revokes: not the administrator, a member of `DB:Admin` nor a
    /// connection that gave no user.
    fn held_to_grant_option(&self) -> Option<&str> {
        self.user()
            .filter(|user| !self.is_admin() && !self.in_db_admin(user))
    }

    /// Whether the connection's user may back up and restore the dictionary
    /// (`sp_BackupDatabase`, `sp_RestoreDatabase`): the administrator, a
    /// member of `DB:Admin` or of `DB:Backup`, or a connection that gave no
    /// user.
    pub(crate) fn may_back_up(&self) -> bool {
        let Some(user) = self.user() else {
            return true;
        };
        self.is_admin()
            || self.dd.named(Kind::User, user).is_some_and(|object| {
                let groups = self.built_in_groups(object);
                groups.contains(&"DB:Admin") || groups.contains(&"DB:Backup")
            })
    }

    /// Whether the connection's user is a member of `DB:Admin` without being the
    /// administrator.
    pub(crate) fn is_admin_member(&self) -> bool {
        self.user()
            .is_some_and(|user| !self.is_admin() && self.in_db_admin(user))
    }

    /// The rights the connection's user may grant or revoke on the object of
    /// `object_type` (0: by name) named `name` (`parent` a column's table), those it
    /// holds with the grant option; `None` where it is not held to it.
    pub(crate) fn grantable(&self, object_type: u16, name: &str, parent: &str) -> Option<u32> {
        let user = self.held_to_grant_option()?;
        Some(crate::system::grantable_mask(self, user, object_type, name, parent).unwrap_or(0))
    }

    /// Whether the connection's user may grant and revoke the right to create
    /// objects of `object_type`: held with the grant option, or not held to it.
    /// Giving it with the grant option (`with_grant`) the original refuses a user
    /// who is held to it, whatever it holds (`fixtures/sql/ddgrantopt`).
    pub(crate) fn may_grant_create(&self, object_type: u16, with_grant: bool) -> bool {
        self.held_to_grant_option().is_none_or(|user| {
            !with_grant && crate::system::grants_create_right(self, user, i64::from(object_type))
        })
    }

    /// The user to give the rights on what it creates: one that is not the
    /// administrator.
    pub(crate) fn creator(&self) -> Option<String> {
        self.user().filter(|_| !self.is_admin()).map(str::to_owned)
    }

    /// `relative`, a path the dictionary keeps relative to its folder
    /// (`.\customers.adt`), as a path.
    pub(crate) fn path(&self, relative: &str) -> PathBuf {
        let mut path = self.folder.clone();
        for part in relative
            .split(['\\', '/'])
            .filter(|p| !p.is_empty() && *p != ".")
        {
            path.push(part);
        }
        path
    }

    /// The table `name` names: a table of the dictionary by its name, or
    /// by its file relative to the dictionary's folder, whatever the case.
    pub(crate) fn table(&self, name: &str) -> Option<Bound> {
        let table = self.dd.named(Kind::Table, name).or_else(|| {
            let wanted = self.path(name);
            self.dd.all(Kind::Table).into_iter().find(|table| {
                let path = self.path(&table.text(dd::table::RELATIVE_PATH).unwrap_or_default());
                same_path(&path, &wanted)
            })
        })?;
        let path = self.path(&table.text(dd::table::RELATIVE_PATH)?);
        let index_files = self
            .dd
            .children(table.id, Kind::IndexFile)
            .into_iter()
            .filter_map(|file| file.text(dd::index_file::RELATIVE_PATH))
            .map(|relative| self.path(&relative))
            .collect();
        let primary_key = self.primary_key(table).map(|index| index.name.clone());
        let table_type = table_type(table);
        Some(Bound {
            path,
            table_type,
            char_type: char_type(table),
            index_files,
            primary_key,
            object_id: table.id,
        })
    }

    /// The names of the dictionary's tables, in its order.
    pub(crate) fn table_names(&self) -> Vec<String> {
        self.dd
            .all(Kind::Table)
            .into_iter()
            .map(|table| table.name.clone())
            .collect()
    }

    /// The `SELECT` of the view `name`, if the dictionary has one.
    pub(crate) fn view(&self, name: &str) -> Option<String> {
        self.dd.named(Kind::View, name)?.text(dd::view::STATEMENT)
    }

    /// The index that is `table`'s primary key.
    pub(crate) fn primary_key(&self, table: &Object) -> Option<&Object> {
        let id = match table_type(table) {
            TableType::Adt | TableType::Database => table.info1.filter(|&id| id > 0)? as u32,
            _ => table.u32(dd::table::PRIMARY_KEY)?,
        };
        self.dd.object(id).filter(|o| o.kind == Kind::Index)
    }

    /// Whether the `DB:Public` rights slot of `object` does not check, which
    /// the original engine answers with errors wherever it reads it
    /// (`fixtures/sql/ddpublicbad`).
    pub(crate) fn public_slot_bad(&self, object: &Object) -> bool {
        self.dd.public_rights(object, self.key().as_deref()) == dd::PublicRights::Invalid
    }

    /// Whether an object of one of `kinds` has a rights slot that does not
    /// check.
    pub(crate) fn any_public_slot_bad(&self, kinds: &[Kind]) -> bool {
        let key = self.key();
        (self.dd.objects().iter())
            .filter(|object| kinds.contains(&object.kind))
            .any(|object| {
                self.dd.public_rights(object, key.as_deref()) == dd::PublicRights::Invalid
            })
    }

    /// Whether the table or view `name` has a rights slot that does not check.
    pub(crate) fn named_public_slot_bad(&self, name: &str) -> bool {
        [Kind::Table, Kind::View].into_iter().any(|kind| {
            self.dd
                .named(kind, name)
                .is_some_and(|object| self.public_slot_bad(object))
        })
    }

    /// Whether what the system tables list depends on the rights of the
    /// user: not for the administrator nor a member of `DB:Admin`, who see
    /// every object (`fixtures/sql/ddpublicbad6`).
    pub(crate) fn lists_by_rights(&self) -> bool {
        self.enforces() && !self.is_admin_member()
    }

    /// Whether the connection's user may see `object` in a listing: always,
    /// unless the dictionary verifies the user's access rights; then a table
    /// or view the user holds some right on, a link the user may use
    /// (probed, `fixtures/sql/linklist.json`).
    pub(crate) fn lists(&self, object: &Object) -> bool {
        use crate::system::{permissions_mask, right};
        let Some(user) = self.user.as_deref().filter(|_| self.lists_by_rights()) else {
            return true;
        };
        match object.kind {
            Kind::Table => self.access_mask(&object.name) & !right::INHERIT != 0,
            Kind::View => permissions_mask(self, user, 6, &object.name, "", true)
                .is_ok_and(|mask| mask & !right::INHERIT != 0),
            Kind::Link => permissions_mask(self, user, 12, &object.name, "", true)
                .is_ok_and(|mask| mask & right::ACCESS != 0),
            _ => true,
        }
    }

    /// Whether the connection's user may see `object` in the system tables
    /// and the catalog procedures: what [`Database::lists`] lets it see, and
    /// the procedures, functions and packages it holds a right on
    /// (`fixtures/sql/ddvisible`).
    pub(crate) fn shows(&self, object: &Object) -> bool {
        use crate::system::{permissions_mask, right};
        let object_type = match object.kind {
            Kind::Procedure => 10,
            Kind::Function => 18,
            Kind::Package => 19,
            _ => return self.lists(object),
        };
        let Some(user) = self.user.as_deref().filter(|_| self.lists_by_rights()) else {
            return true;
        };
        permissions_mask(self, user, object_type, &object.name, "", true)
            .map_or(true, |mask| mask & !right::INHERIT != 0)
    }

    /// Whether the connection's user may see the column `field` of the table
    /// `table` in the system tables: one it holds a right on, itself or by
    /// the table's (`fixtures/sql/ddvisible`).
    pub(crate) fn shows_column(&self, table: &Object, field: &Object) -> bool {
        use crate::system::{permissions_mask, right};
        let Some(user) = self.user.as_deref().filter(|_| self.lists_by_rights()) else {
            return true;
        };
        // What a column grant gives the table is the right to read it, which
        // is not a right on the other columns.
        let held = |object_type, name: &str, parent: &str, except: u32| {
            permissions_mask(self, user, object_type, name, parent, true)
                .is_ok_and(|mask| mask & !except != 0)
        };
        // What the table's slot grants DB:Public reaches every column
        // (`fixtures/sql/ddpublicbad2`).
        let public = matches!(
            self.dd.public_rights(table, self.key().as_deref()),
            dd::PublicRights::Rights(rights) if rights != 0
        );
        public
            || held(4, &field.name, &table.name, right::INHERIT)
            || held(1, &table.name, "", right::INHERIT | right::READ)
    }

    /// What the connection's user may do with the columns of the table
    /// `name`, where it may not do all: those it may not read, which read as
    /// NULL and which a condition may not name, and those it may not update,
    /// by the table's permission level (`fixtures/sql/ddcolrights`). `None`
    /// where it may use them all, which includes a connection that does not
    /// enforce rights.
    pub(crate) fn column_access(&self, name: &str) -> Option<ColumnAccess> {
        use crate::system::{permissions_mask, right};
        let user = self.user.as_deref().filter(|_| self.enforces())?;
        let table = self.dd.named(Kind::Table, name)?;
        if !crate::system::holds_column_rights(self, user, table.id) {
            return None;
        }
        let mut access = ColumnAccess {
            table: table.name.clone(),
            level: permission_level(table),
            lenient: crate::system::group_holds_table_rights(self, user, table.id),
            columns: Vec::new(),
        };
        for field in self.dd.children(table.id, Kind::Field) {
            let mask =
                permissions_mask(self, user, 4, &field.name, &table.name, true).unwrap_or(u32::MAX);
            access.columns.push(ColumnRights {
                name: field.name.clone(),
                read: mask & right::READ != 0,
                update: mask & right::UPDATE != 0,
            });
        }
        Some(access)
    }

    /// The `TABLE_PERMISSION_LEVEL` of the table `name` for the connection's
    /// user, where rights are enforced on it.
    pub(crate) fn permission_level_of(&self, name: &str) -> Option<u8> {
        self.user.as_deref().filter(|_| self.enforces())?;
        self.dd.named(Kind::Table, name).map(permission_level)
    }

    /// Whether the connection's user sees the properties of the object of
    /// `object_type` named `name` in full in the system tables: the ones
    /// that tell how it is made go to a user who may alter it
    /// (`fixtures/sql/ddvisible`).
    pub(crate) fn may_alter(&self, object_type: u16, name: &str) -> bool {
        self.user.is_none() || self.may(object_type, name, crate::system::right::ALTER)
    }

    /// Whether the connection's user sees whether the columns of the table
    /// `table` take `NULL`: a user who may write to the table or alter it,
    /// and everyone where rights are not verified.
    pub(crate) fn may_write(&self, table: &str) -> bool {
        use crate::system::right;
        !self.enforces()
            || self.in_db_admin(self.user().unwrap_or_default())
            || self.access_mask(table)
                & (right::UPDATE | right::INSERT | right::DELETE | right::ALTER | right::DROP)
                != 0
    }

    /// Whether the connection's user sees the dictionary's settings and
    /// relations in full: the administrator does, and so does a member of
    /// `DB:Admin`.
    pub(crate) fn sees_everything(&self) -> bool {
        match self.user() {
            None => true,
            Some(user) => self.is_admin() || self.in_db_admin(user),
        }
    }

    /// Whether the connection's user may see `trigger` in a listing: the
    /// triggers of a table are the user's who may alter the table, whether
    /// or not the dictionary verifies access rights, and the database's own
    /// are the administrator's alone (`fixtures/sql/ddrights`,
    /// `fixtures/sql/ddvisible`).
    pub(crate) fn lists_trigger(&self, trigger: &Object) -> bool {
        self.user.is_none()
            || match self
                .dd
                .object(trigger.parent)
                .filter(|parent| parent.kind == Kind::Table)
            {
                Some(table) => self.may(1, &table.name, crate::system::right::ALTER),
                None => self.sees_everything(),
            }
    }

    /// The tables and views a listing of `ADS_DD_TABLE_VIEW_OR_LINK_OBJECT`
    /// (or `AdsFindFirstTable62`) shows the user, and among them, where its
    /// tables go, the dictionary's links the user may use: `(name, true)`
    /// for a link.
    pub(crate) fn tables_views_and_links(&self) -> Vec<(String, bool)> {
        self.dd
            .objects()
            .iter()
            .filter(|o| matches!(o.kind, Kind::Table | Kind::View | Kind::Link))
            .filter(|o| self.lists(o))
            .map(|o| (o.name.clone(), o.kind == Kind::Link))
            .collect()
    }

    /// The names `AdsDDFindFirstObject` lists for `kind`, under `parent`
    /// (a table, for its fields, index files, indexes and triggers). Tables,
    /// views and links are those the user may see (see [`Database::lists`]);
    /// a listing of [`ObjectType::TableViewOrLink`] has the dictionary's own
    /// alone (the tables of its links are added by `crate::find`).
    pub fn objects(&self, kind: ObjectType, parent: Option<&str>) -> Result<Vec<String>, Error> {
        use ObjectType::*;
        let names = |objects: Vec<&Object>| {
            objects
                .iter()
                .filter(|o| self.lists(o))
                .map(|o| o.name.clone())
                .collect()
        };
        let parent = parent.filter(|p| !p.is_empty());
        let under = |kind| -> Result<Vec<String>, Error> {
            let table = self.dd.named(Kind::Table, parent.unwrap_or_default());
            let table = table.ok_or(Error::InvalidObjectName)?;
            Ok(names(self.dd.children(table.id, kind)))
        };
        let names: Vec<String> = match kind {
            Table => names(self.dd.all(Kind::Table)),
            Relation => names(self.dd.all(Kind::Relation)),
            View => names(self.dd.all(Kind::View)),
            ViewOrTable | TableViewOrLink => self
                .dd
                .objects()
                .iter()
                .filter(|o| matches!(o.kind, Kind::Table | Kind::View) && self.lists(o))
                .map(|o| o.name.clone())
                .collect(),
            User => names(self.dd.all(Kind::User)),
            Group => {
                let mut groups: Vec<String> = names(self.dd.all(Kind::Group));
                groups.extend(dd::BUILT_IN_GROUPS.iter().map(|g| g.to_string()));
                groups
            }
            Procedure => names(self.dd.all(Kind::Procedure)),
            Link => names(self.dd.all(Kind::Link)),
            // Those of the database, not of its packages.
            Function => names(
                self.dd
                    .all(Kind::Function)
                    .into_iter()
                    .filter(|f| {
                        self.dd
                            .object(f.parent)
                            .is_none_or(|p| p.kind != Kind::Package)
                    })
                    .collect(),
            ),
            Package => names(self.dd.all(Kind::Package)),
            Publication => names(self.dd.all(Kind::Publication)),
            // Whichever publication it is asked for, as the original lists them.
            Subscription => names(self.dd.all(Kind::Subscription)),
            // The tables a publication publishes (`fixtures/sql/ddrepl`).
            Article => {
                let parent = parent.ok_or_else(|| crate::dictionary_write::not_named("table"))?;
                let publication = self
                    .dd
                    .named(Kind::Publication, parent)
                    .ok_or_else(|| crate::dictionary_write::not_named("publication"))?;
                self.dd
                    .children(publication.id, Kind::Article)
                    .iter()
                    .filter_map(|article| self.dd.object(article.info1? as u32))
                    .map(|table| table.name.clone())
                    .collect()
            }
            Field if parent.is_some() => under(Kind::Field)?,
            IndexFile if parent.is_some() => under(Kind::IndexFile)?,
            Index if parent.is_some() => under(Kind::Index)?,
            Trigger if parent.is_some() => under(Kind::Trigger)?,
            Trigger => names(self.dd.all(Kind::Trigger)),
            QualifiedTrigger => self
                .dd
                .all(Kind::Trigger)
                .into_iter()
                .map(|t| self.qualified(t))
                .collect(),
            DatabaseTrigger => self
                .dd
                .all(Kind::Trigger)
                .into_iter()
                .filter(|t| !self.on_table(t))
                .map(|t| t.name.clone())
                .collect(),
            Field | IndexFile | Index => return Err(Error::InvalidObjectName),
            Database | Permission => return Err(Error::InvalidObjectType),
        };
        match names.is_empty() {
            true => Err(Error::NoObjectFound),
            false => Ok(names),
        }
    }

    /// Whether `trigger` is a table's (not the database's).
    pub(crate) fn on_table(&self, trigger: &Object) -> bool {
        let table = trigger.info1.and_then(|id| self.dd.object(id as u32));
        table.is_some_and(|t| t.kind == Kind::Table)
    }

    /// The qualified name of `trigger`: `table::trigger`, or its name for a
    /// database trigger.
    pub(crate) fn qualified(&self, trigger: &Object) -> String {
        match trigger.info1.and_then(|id| self.dd.object(id as u32)) {
            Some(table) if table.kind == Kind::Table => format!("{}::{}", table.name, trigger.name),
            _ => trigger.name.clone(),
        }
    }

    /// The object `object` names, if the dictionary has it.
    fn find(&self, object: &ObjectRef) -> Option<&Object> {
        let table = |name: &str| self.dd.named(Kind::Table, name);
        let child =
            |table_name: &str, kind, name: &str| self.dd.child(table(table_name)?.id, kind, name);
        match object {
            ObjectRef::Database => self.dd.database(),
            ObjectRef::Table(name) => table(name),
            ObjectRef::Field(t, name) => child(t, Kind::Field, name),
            ObjectRef::IndexFile(t, name) => child(t, Kind::IndexFile, name),
            ObjectRef::Index(t, name) => child(t, Kind::Index, name),
            ObjectRef::Relation(name) => self.dd.named(Kind::Relation, name),
            ObjectRef::Group(name) => self.dd.named(Kind::Group, name),
            ObjectRef::View(name) => self.dd.named(Kind::View, name),
            ObjectRef::Procedure(name) => self.dd.named(Kind::Procedure, name),
            ObjectRef::User(name) => self.dd.named(Kind::User, name),
            ObjectRef::Link(name) => self.dd.named(Kind::Link, name),
            ObjectRef::Trigger(name) => self
                .dd
                .all(Kind::Trigger)
                .into_iter()
                .find(|t| self.qualified(t).eq_ignore_ascii_case(name)),
            ObjectRef::Publication(name) => self.dd.named(Kind::Publication, name),
            ObjectRef::Article(publication, table) => {
                let publication = self.dd.named(Kind::Publication, publication)?;
                let table = self.dd.named(Kind::Table, table)?;
                self.dd
                    .child(publication.id, Kind::Article, &format!("{:08X}", table.id))
            }
            ObjectRef::Subscription(name) => self.dd.named(Kind::Subscription, name),
        }
    }

    /// The name of the object numbered `id`, if any.
    pub(crate) fn name_of(&self, id: Option<u32>) -> Option<String> {
        id.and_then(|id| self.dd.object(id)).map(|o| o.name.clone())
    }

    /// The names of the groups `user` belongs to beyond the built-in
    /// `DB:Public`, in membership order: its membership property slot decrypted
    /// with the dictionary key, each group object id resolved to its name.
    /// `None` when the user has groups but the key cannot be recovered (the
    /// connection gave no administrator password, or not the right one), so the
    /// caller can tell "no groups" from "cannot read them".
    pub(crate) fn user_groups(&self, user: &Object) -> Option<Vec<String>> {
        let want = user.info1.unwrap_or(0).max(0) as usize;
        if want == 0 {
            return Some(Vec::new());
        }
        let key = self.key()?;
        let slot = user.slot(dd::user::GROUP_MEMBERSHIP)?;
        let mut data = slot.to_vec();
        tflip_storage::crypto::group_membership(self.scheme(), &key, &mut data);
        let names: Vec<String> = data
            .chunks(4)
            .filter(|c| c.len() == 4)
            .filter_map(|c| self.dd.object(u32::from_le_bytes(c.try_into().unwrap())))
            .map(|o| o.name.clone())
            .collect();
        (names.len() == want).then_some(names)
    }

    /// The path the dictionary keeps relative to its folder in `relative`,
    /// full, ending in a separator for a folder.
    pub(crate) fn full_path(&self, relative: &str, folder: bool) -> String {
        let mut path = self.path(relative).to_string_lossy().into_owned();
        if folder && !path.ends_with(MAIN_SEPARATOR) {
            path.push(MAIN_SEPARATOR);
        }
        path
    }

    /// The property `id` (`ADS_DD_*`) of `object`.
    ///
    /// The common properties (comment, user-defined property, triggers
    /// disabled) apply to every object; the others to the object of their
    /// range (100s the database, 200s tables, ...). A property the object
    /// does not have is [`Error::PropertyNotSet`]; an `id` not of its kind
    /// [`Error::InvalidPropertyId`]; an object the dictionary does not have
    /// [`Error::InvalidObjectName`].
    pub fn property(&self, object: &ObjectRef, id: u16) -> Result<Property, Error> {
        if let ObjectRef::Group(name) = object
            && dd::built_in_group(name).is_some()
        {
            return match id {
                1 | 3 => Err(Error::PropertyNotSet),
                _ => Err(Error::InvalidPropertyId),
            };
        }
        // As in the original engine, a property the kind of object does
        // not have fails before the object is looked for; a referential
        // integrity rule is looked for first, whatever the property.
        let relation = matches!(object, ObjectRef::Relation(_));
        if !relation && !has_property(object, id) {
            return Err(Error::InvalidPropertyId);
        }
        let found = self.find(object).ok_or(Error::InvalidObjectName);
        // The built-in administrator has no properties to read, though its
        // name is one the dictionary does not have.
        if let ObjectRef::User(name) = object
            && name.eq_ignore_ascii_case("ADSSYS")
        {
            return Err(Error::InvalidPropertyId);
        }
        let found = found?;
        if relation && !has_property(object, id) {
            return Err(Error::InvalidPropertyId);
        }
        let text = |text: Option<String>| text.map(Property::Text).ok_or(Error::PropertyNotSet);
        let wide = |text: Option<String>| text.map(Property::Wide).ok_or(Error::PropertyNotSet);
        let set = |value: Option<Property>| value.ok_or(Error::PropertyNotSet);
        let flag = |on: bool| Ok(Property::U16(u16::from(on)));
        match (object, id) {
            // The original looks for the object before it refuses these.
            (ObjectRef::Link(_), 1 | 3) | (ObjectRef::Article(..), 3) => {
                Err(Error::InvalidPropertyId)
            }
            // ADS_DD_COMMENT, ADS_DD_USER_DEFINED_PROP.
            (_, 1) => text(found.comment.clone().map(|c| match found.comment_nul {
                true => c + "\0",
                false => c,
            })),
            (_, 3) => text(self.user_defined(object, found)),
            // ADS_DD_TRIGGERS_DISABLED.
            (
                ObjectRef::Database
                | ObjectRef::Table(_)
                | ObjectRef::View(_)
                | ObjectRef::Trigger(_),
                5,
            ) => flag(match object {
                ObjectRef::Table(_) => found.has(dd::table::TRIGGERS_DISABLED),
                ObjectRef::View(_) => found.has(dd::view::TRIGGERS_DISABLED),
                ObjectRef::Trigger(_) => found.has(dd::trigger::TRIGGERS_DISABLED),
                _ => found.has(dd::database::TRIGGERS_DISABLED),
            }),
            (ObjectRef::Database, id) => self.database_property(found, id),
            (ObjectRef::Table(_), id) => self.table_property(found, id),
            (ObjectRef::Field(..), id) => field_property(found, id),
            (ObjectRef::IndexFile(..), id) => match id {
                900 => {
                    let relative = found.text(dd::index_file::RELATIVE_PATH);
                    text(relative.map(|r| self.full_path(&r, false)))
                }
                901 => set(found.u32(dd::index_file::PAGE_SIZE).map(Property::U32)),
                902 => text(found.text(dd::index_file::RELATIVE_PATH)),
                903 => set(found.info1.map(|t| Property::U8(t as u8))),
                _ => Err(Error::InvalidPropertyId),
            },
            (ObjectRef::Index(..), id) => self.index_property(found, id),
            (ObjectRef::Relation(_), id) => {
                use dd::relation::*;
                match id {
                    500 => set(Some(Property::U32(found.id))),
                    501 => text(self.name_of(found.u32(PRIMARY_TABLE))),
                    502 => text(self.name_of(found.u32(PRIMARY_INDEX))),
                    503 => text(self.name_of(found.u32(FOREIGN_TABLE))),
                    504 => text(self.name_of(found.u32(FOREIGN_INDEX))),
                    505 => set(found.u8(UPDATE_RULE).map(Property::U8)),
                    506 => set(found.u8(DELETE_RULE).map(Property::U8)),
                    507 => text(found.text(NO_PKEY_ERROR)),
                    508 => text(found.text(CASCADE_ERROR)),
                    _ => Err(Error::InvalidPropertyId),
                }
            }
            // The rights are kept with the objects they are on, in the
            // dictionary's order of tables, views and procedures.
            (ObjectRef::Group(name), 1001..=1004) => {
                let tables = (Kind::Table, 1);
                let views = (Kind::View, 6);
                let procedures = (Kind::Procedure, 10);
                let kinds = match id {
                    1001 => vec![tables],
                    1002 => vec![views],
                    1003 => vec![procedures],
                    _ => vec![tables, views, procedures],
                };
                crate::system::group_rights_text(self, name, &kinds)
                    .map(Property::Text)
                    .ok_or(Error::NotImplemented)
            }
            (ObjectRef::Group(_), 1005) => Err(Error::PropertyNotSet),
            (ObjectRef::Group(_), _) => Err(Error::InvalidPropertyId),
            (ObjectRef::View(_), id) => match id {
                700 => text(found.text(dd::view::STATEMENT)),
                701 => set(found.u16(dd::view::STATEMENT_LENGTH).map(Property::U16)),
                702 | 703 => Err(Error::PropertyNotSet),
                704 => wide(found.text(dd::view::STATEMENT)),
                _ => Err(Error::InvalidPropertyId),
            },
            (ObjectRef::Procedure(_), id) => {
                use dd::procedure::*;
                match id {
                    800 => text(found.text(INPUT)),
                    801 => text(found.text(OUTPUT)),
                    802 => text(found.text(DLL_NAME)),
                    803 => text(found.text(DLL_FUNCTION_NAME)),
                    // As stored, as the other properties of a procedure.
                    804 => set(found
                        .slot(INVOKE_OPTION)
                        .map(|b| Property::Bytes(b.to_vec()))),
                    805 => text(found.text(SCRIPT)),
                    806 => wide(found.text(SCRIPT)),
                    _ => Err(Error::InvalidPropertyId),
                }
            }
            (ObjectRef::User(_), id) => match id {
                // The member groups (decrypted with the dictionary key), then
                // the built-in DB:Public every user is in, joined with ';', as
                // the engine returns them. NotImplemented when the user has
                // groups but the key cannot be recovered.
                1102 => match self.user_groups(found) {
                    Some(mut names) => {
                        names.push("DB:Public".to_owned());
                        text(Some(names.join(";")))
                    }
                    None => Err(Error::NotImplemented),
                },
                1105 => flag(!found.has(dd::user::NO_OLD_PASSWORD)),
                // The rights to free tables are one no call sets, so none is
                // ever set (`fixtures/dictionary/freetables.json`).
                1005 => Err(Error::PropertyNotSet),
                _ => Err(Error::InvalidPropertyId),
            },
            (ObjectRef::Link(_), id) => match id {
                1300 => {
                    let relative = found.text(dd::link::RELATIVE_PATH);
                    text(relative.map(|r| self.full_path(&r, false)))
                }
                1301 => set(found.info1.map(|o| Property::U32(o as u32))),
                1302 => text(found.text(dd::link::USER_NAME)),
                1303 => text(found.text(dd::link::RELATIVE_PATH)),
                _ => Err(Error::InvalidPropertyId),
            },
            (ObjectRef::Trigger(_), id) => {
                use dd::trigger::*;
                let number = |slot| set(found.u32(slot).map(Property::U32));
                match id {
                    1400 => set(found.info1.map(|t| Property::U32(t as u32))),
                    1401 => number(EVENT_TYPE),
                    1402 => number(TRIGGER_TYPE),
                    1403 => number(CONTAINER_TYPE),
                    1404 => text(found.text(CONTAINER)),
                    1405 => text(found.text(FUNCTION_NAME)),
                    1406 => number(PRIORITY),
                    1407 => number(OPTIONS),
                    1408 => text(
                        self.on_table(found)
                            .then(|| self.name_of(found.info1.map(|t| t as u32)))
                            .flatten(),
                    ),
                    1409 => wide(found.text(CONTAINER)),
                    _ => Err(Error::InvalidPropertyId),
                }
            }
            (ObjectRef::Publication(_), id) => match id {
                // ADS_DD_PUBLICATION_OPTIONS.
                1500 => set(found.u32(dd::publication::OPTIONS).map(Property::U32)),
                _ => Err(Error::InvalidPropertyId),
            },
            (ObjectRef::Article(..), id) => {
                use dd::article::*;
                // The include or exclude list, by the kind flag in slot 4.
                let list = |want: u16| {
                    (found.u16(LIST_KIND) == Some(want))
                        .then(|| found.text(COLUMN_LIST))
                        .flatten()
                };
                match id {
                    1600 => text(found.text(FILTER)),
                    1601 => text(found.text(ID_COLUMNS)),
                    // ADS_DD_ARTICLE_ID_COLUMN_NUMBERS: the id columns' numbers.
                    1602 => set(found
                        .text(ID_COLUMNS)
                        .map(|c| Property::Bytes(self.column_numbers(found, &c)))),
                    1603 => text(found.text(FILTER_SHORT)),
                    1604 => text(list(LIST_INCLUDE)),
                    1605 => text(list(LIST_EXCLUDE)),
                    // ADS_DD_ARTICLE_INC_COLUMN_NUMBERS: the include columns'
                    // numbers, or the id columns' when there is no include list.
                    1606 => {
                        let columns = list(LIST_INCLUDE).or_else(|| found.text(ID_COLUMNS));
                        set(columns.map(|c| Property::Bytes(self.column_numbers(found, &c))))
                    }
                    1607 => flag(found.has(INSERT_MERGE)),
                    1608 => flag(found.has(UPDATE_MERGE)),
                    1609 => wide(found.text(FILTER)),
                    _ => Err(Error::InvalidPropertyId),
                }
            }
            (ObjectRef::Subscription(_), id) => {
                use dd::subscription::*;
                // A deciphered secret (password or connection string): the key,
                // the slot's bytes, the subscription's record.
                let secret = |slot: usize| -> Option<String> {
                    let key = self.key()?;
                    let data = found.slot(slot)?;
                    let text = tflip_storage::crypto::decipher_subscription_secret(
                        self.scheme(),
                        &key,
                        found.record(),
                        slot as u32,
                        data,
                    );
                    Some(String::from_utf8_lossy(&text).into_owned())
                };
                // A communication-type bit of the options dword.
                let comm_bit = |bit: u32| flag(found.u32(OPTIONS).unwrap_or(0) & bit != 0);
                match id {
                    1700 => text(self.name_of(found.info1.map(|p| p as u32))),
                    1701 => text(found.text(TARGET)),
                    1702 => text(found.text(USER_NAME)),
                    1703 => secret(PASSWORD)
                        .map(Property::Text)
                        .ok_or(Error::PropertyNotSet),
                    1704 => set(found.u16(FORWARD).map(Property::U16)),
                    1705 => set(found.u16(ENABLED).map(Property::U16)),
                    // The queue, made absolute against the dictionary's folder.
                    1706 => text(found.text(QUEUE_NAME).map(|q| self.full_path(&q, false))),
                    // ADS_DD_SUBSCR_OPTIONS: the options without the comm bits.
                    1707 => set(found.u32(OPTIONS).map(|o| Property::U32(o & !comm::MASK))),
                    1708 => text(found.text(QUEUE_NAME)),
                    1709 => set(found.u16(PAUSED).map(Property::U16)),
                    1710 => comm_bit(comm::TCP_IP),
                    1711 => comm_bit(comm::TCP_IP_V6),
                    1712 => comm_bit(comm::UDP_IP),
                    1713 => comm_bit(comm::IPX),
                    // ADS_DD_SUBSCR_OPTIONS_INTERNAL: the whole options dword.
                    1714 => set(found.u32(OPTIONS).map(Property::U32)),
                    1715 => comm_bit(comm::TLS),
                    1716 => secret(CONNECTION_STR)
                        .map(Property::Text)
                        .ok_or(Error::PropertyNotSet),
                    _ => Err(Error::InvalidPropertyId),
                }
            }
        }
    }

    /// The field numbers of the `;`-separated `columns` of `article`'s table,
    /// each a little-endian `u32`, as the engine returns an article's column
    /// number arrays. A column that is no field of the table is left out.
    fn column_numbers(&self, article: &Object, columns: &str) -> Vec<u8> {
        let fields: std::collections::HashMap<String, i32> = article
            .info1
            .and_then(|id| self.dd.object(id as u32))
            .map(|table| {
                self.dd
                    .children(table.id, Kind::Field)
                    .iter()
                    .filter_map(|f| f.info1.map(|n| (f.name.to_ascii_lowercase(), n)))
                    .collect()
            })
            .unwrap_or_default();
        columns
            .split(';')
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .filter_map(|c| fields.get(&c.to_ascii_lowercase()))
            .flat_map(|&n| (n as u32).to_le_bytes())
            .collect()
    }

    /// The user-defined property of `object`.
    fn user_defined(&self, object: &ObjectRef, found: &Object) -> Option<String> {
        let slot = match object {
            ObjectRef::Database => dd::database::USER_DEFINED,
            ObjectRef::Table(_) => dd::table::USER_DEFINED,
            ObjectRef::Field(..) => dd::field::USER_DEFINED,
            ObjectRef::User(_) => dd::user::USER_DEFINED,
            ObjectRef::Publication(_) => dd::publication::USER_DEFINED,
            ObjectRef::Subscription(_) => dd::subscription::USER_DEFINED,
            _ => return None,
        };
        found.text(slot)
    }

    fn database_property(&self, db: &Object, id: u16) -> Result<Property, Error> {
        use dd::database::*;
        let text = |slot| {
            stored_text(db, slot)
                .map(Property::Text)
                .ok_or(Error::PropertyNotSet)
        };
        let u16_or = |slot, default| Ok(Property::U16(db.u16(slot).unwrap_or(default)));
        let flag = |on: bool| Ok(Property::U16(u16::from(on)));
        let folder = |slot| {
            let relative = db.text(slot).unwrap_or_default();
            Ok(Property::Text(self.full_path(&relative, true)))
        };
        let relative = |slot| match db.text(slot) {
            Some(relative) => Ok(Property::Text(relative)),
            // `.\`, without its NUL.
            None => Ok(Property::Bytes(b".\\".to_vec())),
        };
        match id {
            2 => Ok(Property::U32(db.u32(VERSION).unwrap_or(0))),
            100 => folder(DEFAULT_TABLE_PATH),
            102 => folder(TEMP_TABLE_PATH),
            103 => flag(!db.has(LOGIN_NOT_REQUIRED)),
            104 => flag(!db.has(RIGHTS_NOT_CHECKED)),
            105 => Err(Error::PropertyNotSet),
            106 => flag(db.has(ENCRYPT_NEW_TABLE)),
            107 => flag(db.u16(ENABLE_INTERNET) == Some(1)),
            108 => u16_or(INTERNET_SECURITY_LEVEL, 2),
            109 => u16_or(MAX_FAILED_ATTEMPTS, 5),
            110 => flag(false),
            111 => u16_or(VERSION_MAJOR, 0),
            112 => u16_or(VERSION_MINOR, 0),
            113 => flag(db.u16(LOGINS_DISABLED) == Some(1)),
            114 => text(LOGINS_DISABLED_MESSAGE),
            115 => text(FTS_DELIMITERS),
            116 => text(FTS_NOISE),
            117 => text(FTS_DROP_CHARS),
            118 => text(FTS_CONDITIONAL_CHARS),
            119 => flag(false),
            120 => flag(db.has(ENCRYPT_INDEXES)),
            121 => text(QUERY_LOG_TABLE),
            122 => flag(db.has(ENCRYPT_COMMUNICATION)),
            123 => relative(DEFAULT_TABLE_PATH),
            124 => relative(TEMP_TABLE_PATH),
            125 => flag(db.has(DISABLE_DLL_CACHING)),
            126 => Ok(Property::U32(db.u32(DATA_ENCRYPTION_TYPE).unwrap_or(3))),
            131 => Ok(Property::U32(db.u32(QUERY_VIA_ROOT).unwrap_or(0))),
            132 => flag(db.u16(ENFORCE_MAX_FAILED_LOGINS) == Some(1)),
            _ => Err(Error::InvalidPropertyId),
        }
    }

    fn table_property(&self, table: &Object, id: u16) -> Result<Property, Error> {
        use dd::table::*;
        let text = |slot| {
            stored_text(table, slot)
                .map(Property::Text)
                .ok_or(Error::PropertyNotSet)
        };
        let flags = table.info2.unwrap_or(0);
        let flag = |on: bool| Ok(Property::U16(u16::from(on)));
        let stored = |slot| {
            let value = table.u32(slot).ok_or(Error::PropertyNotSet)?;
            Ok(Property::Bytes(value.to_le_bytes().to_vec()))
        };
        match id {
            200 => text(VALIDATION_EXPR),
            201 => text(VALIDATION_MSG),
            202 => self
                .primary_key(table)
                .map(|index| Property::Text(index.name.clone()))
                .ok_or(Error::PropertyNotSet),
            203 => flag(!table.has(NO_AUTO_CREATE)),
            204 => Ok(Property::U16(ads_table_type(table))),
            205 => {
                let relative = table.text(RELATIVE_PATH).unwrap_or_default();
                Ok(Property::Text(self.full_path(&relative, false)))
            }
            // As stored.
            206 => Ok(Property::Bytes(
                table.u16(FIELD_COUNT).unwrap_or(0).to_le_bytes().to_vec(),
            )),
            207 => Ok(Property::U32(table.u32(RI_GRAPH).unwrap_or(0))),
            208 => Ok(Property::U32(table.id)),
            209 => Err(Error::PropertyNotSet),
            210 => flag(
                self.dd
                    .all(Kind::Relation)
                    .iter()
                    .any(|rule| rule.u32(dd::relation::PRIMARY_TABLE) == Some(table.id)),
            ),
            211 => text(RELATIVE_PATH),
            212 => Ok(Property::U16(char_type(table))),
            213 => text(DEFAULT_INDEX),
            214 => flag(flags & dd::table::flags::ENCRYPTED != 0),
            215 => Ok(table
                .u16(MEMO_BLOCK_SIZE)
                .map_or(Property::Unset(2), Property::U16)),
            216 => Ok(Property::U16(match flags {
                f if f & flags::PERMISSION_LEVEL_1 != 0 => 1,
                f if f & flags::PERMISSION_LEVEL_3 != 0 => 3,
                _ => 2,
            })),
            217 => stored(TRIGGER_TYPES),
            218 => stored(TRIGGER_OPTIONS),
            219 => Ok(Property::U16((flags >> flags::CACHING_SHIFT & 3) as u16)),
            220 => flag(flags & flags::TRANSACTION_FREE != 0),
            221 => table
                .text(VALIDATION_EXPR)
                .map(Property::Wide)
                .ok_or(Error::PropertyNotSet),
            222 => flag(flags & flags::WEB_DELTA != 0),
            223 => flag(flags & flags::CONCURRENCY != 0),
            _ => Err(Error::InvalidPropertyId),
        }
    }

    fn index_property(&self, index: &Object, id: u16) -> Result<Property, Error> {
        use dd::index::*;
        let text = |slot| {
            stored_text(index, slot)
                .map(Property::Text)
                .ok_or(Error::PropertyNotSet)
        };
        let u16_of = |slot| Ok(index.u16(slot).map_or(Property::Unset(2), Property::U16));
        match id {
            400 => self
                .name_of(index.info1.map(|f| f as u32))
                .map(Property::Text)
                .ok_or(Error::PropertyNotSet),
            401 => text(EXPRESSION),
            402 => text(CONDITION),
            403 => {
                let options = index.u32(OPTIONS).unwrap_or(0);
                // A unique ADI order is reported a candidate too.
                Ok(Property::U32(if options & 1 != 0 {
                    options | 0x800
                } else {
                    options
                }))
            }
            404 => u16_of(KEY_LENGTH),
            405 => u16_of(KEY_TYPE),
            406 => u16_of(FTS_MIN_LENGTH),
            407 => text(FTS_DELIMITERS),
            408 => text(FTS_NOISE),
            409 => text(FTS_DROP_CHARS),
            410 => text(FTS_CONDITIONAL_CHARS),
            411 => text(COLLATION),
            412 => index
                .text(FTS_DELIMITERS)
                .map(Property::Wide)
                .ok_or(Error::PropertyNotSet),
            413 => index
                .text(FTS_NOISE)
                .map(Property::Wide)
                .ok_or(Error::PropertyNotSet),
            414 => index
                .text(FTS_DROP_CHARS)
                .map(Property::Wide)
                .ok_or(Error::PropertyNotSet),
            415 => {
                let text = index.text(FTS_CONDITIONAL_CHARS);
                text.map(Property::Wide).ok_or(Error::PropertyNotSet)
            }
            _ => Err(Error::InvalidPropertyId),
        }
    }
}

fn field_property(field: &Object, id: u16) -> Result<Property, Error> {
    use dd::field::*;
    let text = |slot| {
        stored_text(field, slot)
            .map(Property::Text)
            .ok_or(Error::PropertyNotSet)
    };
    let wide = |slot| {
        field
            .text(slot)
            .map(Property::Wide)
            .ok_or(Error::PropertyNotSet)
    };
    let definition = field.slot(DEFINITION).unwrap_or_default();
    let part = |i: usize| {
        definition
            .get(2 * i..2 * i + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .unwrap_or(0)
    };
    match id {
        300 => text(DEFAULT_VALUE),
        301 => Ok(Property::U16(u16::from(!field.has(NOT_NULL)))),
        302 => text(MIN_VALUE),
        303 => text(MAX_VALUE),
        304 => text(VALIDATION_MSG),
        305 => Ok(Property::Bytes(definition.to_vec())),
        306 => Ok(Property::U16(part(0))),
        307 => Ok(Property::U16(part(1))),
        308 => Ok(Property::U16(part(2))),
        309 => Ok(Property::U32(field.info1.unwrap_or(0) as u32)),
        310 => Ok(Property::U32(field.u32(OPTIONS).unwrap_or(0))),
        311 => wide(DEFAULT_VALUE),
        312 => wide(MIN_VALUE),
        313 => wide(MAX_VALUE),
        _ => Err(Error::InvalidPropertyId),
    }
}

/// The text in `object`'s slot `slot`, ending in a NUL if it is UTF-8 (see
/// [`Property::Text`]).
fn stored_text(object: &Object, slot: usize) -> Option<String> {
    let text = object.text(slot)?;
    Some(if object.is_utf8(slot) {
        text + "\0"
    } else {
        text
    })
}

/// Whether objects of `object`'s kind have the property `id`, as the
/// original engine answers: the common properties (comment, user-defined
/// property, triggers disabled) of the kinds that have them, and those of
/// the kind's range (100s the database, 200s tables, ...) that the probes
/// found. Passwords are left out.
fn has_property(object: &ObjectRef, id: u16) -> bool {
    match object {
        ObjectRef::Database => matches!(id, 1 | 2 | 3 | 5 | 100 | 102..=126 | 131 | 132),
        ObjectRef::Table(_) => matches!(id, 1 | 3 | 5 | 200..=223),
        ObjectRef::Field(..) => matches!(id, 1 | 3 | 300..=313),
        ObjectRef::IndexFile(..) => matches!(id, 900..=903),
        ObjectRef::Index(..) => matches!(id, 1 | 400..=415),
        ObjectRef::Relation(_) => matches!(id, 500..=508),
        ObjectRef::Group(_) => matches!(id, 1 | 3 | 1001..=1005),
        ObjectRef::View(_) => matches!(id, 1 | 5 | 700..=704),
        ObjectRef::Procedure(_) => matches!(id, 1 | 800..=806),
        ObjectRef::User(_) => matches!(id, 1 | 3 | 1005 | 1102 | 1105),
        ObjectRef::Link(_) => matches!(id, 1..=3 | 1300..=1303),
        ObjectRef::Trigger(_) => matches!(id, 1 | 5 | 1400..=1409),
        ObjectRef::Publication(_) => matches!(id, 1 | 3 | 1500),
        ObjectRef::Article(..) => matches!(id, 1 | 1600..=1609),
        ObjectRef::Subscription(_) => matches!(id, 1 | 3 | 1700..=1716),
    }
}

/// Whether `a` and `b` name the same file, whatever the case.
fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy()),
    }
}
