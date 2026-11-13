//! Links to other data dictionaries, as statements reach their tables:
//! `link.table` in a `SELECT`, `INSERT`, `UPDATE` or `DELETE`.
//!
//! As the original engine does it (probed, `fixtures/sql/links.json`,
//! `tools/ace/sql/links.txt`):
//!
//! - The name before the dot is a link of the connection's dictionary (any
//!   case), or one the connection made for itself (`AdsDDCreateLink` without
//!   `ADS_LINK_GLOBAL`: no other connection sees it, and it is not written to
//!   the dictionary); else it is taken as the path of a dictionary, relative
//!   to the connection's dictionary's folder (`"..\other\other.add".table`).
//!   On a free connection any such name fails (5041).
//! - The linked dictionary is logged into as the link's user with its
//!   password, or — for a link that authenticates as the active user
//!   (`ADS_LINK_AUTH_ACTIVE_USER`), and for a path — as the connection's own
//!   user. Failing that fails the statement with 5174 naming the link and the
//!   reason (7041 no such file, 5100 no such user, 5097 a wrong password),
//!   except that an anonymous login a dictionary refuses is 5054 itself.
//! - A user of a dictionary that verifies access rights needs the link's
//!   `ACCESS` right (5054 without it); the administrator does not.
//! - Only the linked dictionary's tables are reached, not its views (5132).
//!   They open as that dictionary's, for the link's user: its rights, its
//!   referential integrity and its triggers hold. Their writes are not part
//!   of the connection's transaction.
//!
//! The linked dictionary is opened once per connection and link, as a
//! connection of its own that closes with the connection.
//!
//! Outside SQL (probed, `fixtures/sql/linklist.json`, `tools/ace/sql/linklist.txt`):
//!
//! - A listing of `ADS_DD_TABLE_VIEW_OR_LINK_OBJECT`, and `AdsFindFirstTable62`
//!   without a mask, list the tables (not the views) of each global link the
//!   user may use, where the link is among the dictionary's objects:
//!   `:link::table` as an object, the table and the link's name as a table.
//!   The linked dictionary is logged into as the link's user, whose rights
//!   decide what it lists; a link that does not log in is left out. The
//!   listing leaves the global links it followed inactive.
//! - No table opens through a link by name (`AdsOpenTable`: 5132).
//! - The connection's active links (`AdsGetNumActiveLinks`,
//!   `AdsGetActiveLinkInfo`), newest first: a link made on the connection
//!   that logs in, and one a statement uses (a dictionary's path as a link
//!   without a name, unless the dictionary is active as the same user).
//!   Dropping a link makes it inactive.

use std::path::{Component, Path, PathBuf};

use tflip_sql::SqlError;
use tflip_storage::dd::{self, Kind};

use crate::dictionary::Database;
use crate::handle::Handle;
use crate::{Error, Session, connection, lock};

/// `ADS_LINK_GLOBAL`: the link is the dictionary's, for every connection.
pub const GLOBAL: u32 = 0x1;
/// `ADS_LINK_AUTH_ACTIVE_USER`: the link logs in as the connection's user.
pub const AUTH_ACTIVE_USER: u32 = 0x2;
/// `ADS_LINK_PATH_IS_STATIC`: the link keeps its dictionary's full path.
pub const PATH_IS_STATIC: u32 = 0x4;

/// A link a connection made for itself.
#[derive(Debug, Clone)]
pub(crate) struct Local {
    pub name: String,
    /// The linked dictionary, as given.
    pub path: String,
    pub user: String,
    pub password: String,
    pub options: u32,
}

/// A link a connection has active.
#[derive(Debug, Clone)]
pub(crate) struct Active {
    /// The link's name; empty for a dictionary's path.
    pub name: String,
    /// The user it logged in as; empty for none.
    pub user: String,
    /// The linked dictionary.
    pub path: PathBuf,
}

/// Makes `link` active on `owner`, the newest, unless it is already: a link
/// by its name, a dictionary's path by the dictionary and the user.
pub(crate) fn activate(owner: &connection::Connection, link: Active) {
    let mut active = lock(&owner.active_links);
    let known = active.iter().any(|a| match link.name.is_empty() {
        true => a.path == link.path && a.user.eq_ignore_ascii_case(&link.user),
        false => a.name.eq_ignore_ascii_case(&link.name),
    });
    if !known {
        active.insert(0, link);
    }
}

/// Makes the link `name` inactive on `owner`.
pub(crate) fn deactivate(owner: &connection::Connection, name: &str) {
    lock(&owner.active_links).retain(|a| a.name.is_empty() || !a.name.eq_ignore_ascii_case(name));
}

/// The links `connection` has active, newest first: each one's name (empty for
/// a dictionary's path), user and dictionary (`AdsGetActiveLinkInfo`).
pub fn active(
    session: &Session,
    connection: Handle,
) -> Result<Vec<(String, String, String)>, Error> {
    let owner = connection::get(session, connection)?;
    let active = lock(&owner.active_links);
    Ok(active
        .iter()
        .map(|a| (a.name.clone(), a.user.clone(), a.path.display().to_string()))
        .collect())
}

/// The tables and views of `owner`'s dictionary a listing of
/// `ADS_DD_TABLE_VIEW_OR_LINK_OBJECT` shows, with the tables of its links
/// where the links are: `(name, None)` for its own, `(table, Some(link))` for
/// a link's (see the module's documentation). The global links it followed
/// are left inactive.
pub(crate) fn listing(
    session: &Session,
    owner: &connection::Connection,
) -> Result<Vec<(String, Option<String>)>, Error> {
    let entries = owner.with_database(|db| Ok(db.tables_views_and_links()))?;
    let mut names = Vec::new();
    for (name, is_link) in entries {
        if !is_link {
            names.push((name, None));
            continue;
        }
        let tables = target(session, owner, &name, "")
            .and_then(|target| login(&name, "", &target))
            .and_then(|database| database.objects(crate::dictionary::ObjectType::Table, None));
        names.extend(
            tables
                .unwrap_or_default()
                .into_iter()
                .map(|t| (t, Some(name.clone()))),
        );
        deactivate(owner, &name);
    }
    Ok(names)
}

/// Where a link leads and as whom it logs in.
struct Target {
    path: PathBuf,
    user: Option<String>,
    password: Vec<u8>,
}

/// The connection to the dictionary `link` reaches from `connection`, which
/// has the table `table` (see the module's documentation).
pub(crate) fn open(
    session: &Session,
    connection: Handle,
    link: &str,
    table: &str,
) -> Result<Handle, Error> {
    let owner = connection::get(session, connection)?;
    if !owner.has_database() {
        let detail = format!("{link} - link object is not supported on free connection.");
        return Err(ace(
            5041,
            "The requested object was not found.",
            &detail,
            table,
        ));
    }
    let key = link.to_uppercase();
    let opened = lock(&owner.linked)
        .iter()
        .find(|(name, _)| *name == key)
        .map(|&(_, h)| h);
    let linked = match opened {
        Some(handle) => handle,
        None => {
            let target = target(session, &owner, link, table)?;
            let database = login(link, table, &target)?;
            let user = target.user.as_deref();
            let handle = connection::open_linked(session, connection, target.path, database, user);
            lock(&owner.linked).push((key, handle));
            handle
        }
    };
    let linked_connection = connection::get(session, linked)?;
    activate(
        &owner,
        Active {
            name: shown(&owner, link),
            user: linked_connection.user().unwrap_or_default().to_owned(),
            path: linked_connection.path.clone(),
        },
    );
    let found = linked_connection
        .with_database_opt(|db| db.dictionary().named(Kind::Table, table).is_some())
        .unwrap_or(false);
    if !found {
        let detail = format!("Cannot find table \"{table}\" on the specified link.");
        return Err(ace(
            5132,
            "The specified database object name is not valid.",
            &detail,
            table,
        ));
    }
    Ok(linked)
}

/// The name `link` is active under on `owner`: a link's own name, or empty
/// for a dictionary's path.
fn shown(owner: &connection::Connection, link: &str) -> String {
    if let Some(local) = lock(&owner.links)
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(link))
    {
        return local.name.clone();
    }
    owner
        .with_database_opt(|db| {
            db.dictionary()
                .named(Kind::Link, link)
                .map(|o| o.name.clone())
        })
        .flatten()
        .unwrap_or_default()
}

/// Whether the connection's user may make a global link in `db`: the
/// administrator, and a member of `DB:Admin`, whether or not the dictionary
/// verifies access rights (tflip does not read the right to create links,
/// which it cannot grant either).
pub(crate) fn may_create(db: &Database) -> bool {
    let Some(user) = db.user().filter(|_| !db.is_admin()) else {
        return true;
    };
    let dd = db.dictionary();
    db.is_admin_member()
        || dd
            .named(Kind::User, user)
            .and_then(|user| db.user_groups(user))
            .is_some_and(|groups| groups.iter().any(|g| g.eq_ignore_ascii_case("DB:Admin")))
}

/// Makes the link `name` that `owner` just made active, if it logs in to
/// `database` (at `path`): as `user` with `password`, or with
/// `ADS_LINK_AUTH_ACTIVE_USER` in `options` as the connection's user. A wrong
/// password is refused (5097) for a link of the connection's own, while a
/// global link is made all the same, inactive (probed,
/// `fixtures/sql/linklist.json`).
pub(crate) fn activate_made(
    owner: &connection::Connection,
    name: &str,
    database: &Database,
    path: &Path,
    (user, password, options): (&str, &str, u32),
) -> Result<(), Error> {
    let (user, password) = match options & AUTH_ACTIVE_USER {
        0 => (Some(user.to_owned()), password.as_bytes().to_vec()),
        _ => {
            let password = owner
                .with_database_opt(|db| db.password().to_vec())
                .unwrap_or_default();
            (owner.user().map(str::to_owned), password)
        }
    };
    let user = user.filter(|u| !u.is_empty());
    if database.login(user.as_deref(), &password).is_err() {
        return match options & GLOBAL {
            0 => Err(Error::Sql(SqlError::ace(
                5097,
                "The password given is not valid.",
                "",
            ))),
            _ => Ok(()),
        };
    }
    let user = user.unwrap_or_default();
    activate(
        owner,
        Active {
            name: name.to_owned(),
            user,
            path: path.to_path_buf(),
        },
    );
    Ok(())
}

/// Where `link` leads from the dictionary of `owner`, and as whom it logs
/// in; refused without the right to use it.
fn target(
    session: &Session,
    owner: &connection::Connection,
    link: &str,
    table: &str,
) -> Result<Target, Error> {
    // The connection's own user and password, for a link that logs in as the
    // active user and for a path.
    let active = |db: &Database| (owner.user().map(str::to_owned), db.password().to_vec());
    let local = lock(&owner.links)
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(link))
        .cloned();
    let found = owner.with_database(|db| {
        let (relative, user, password, options) = match &local {
            Some(local) => (
                local.path.clone(),
                Some(local.user.clone()),
                local.password.as_bytes().to_vec(),
                local.options,
            ),
            None => match db.dictionary().named(Kind::Link, link) {
                Some(object) => {
                    if !may_use(db, &object.name) {
                        let detail = "Access to the link is not granted.";
                        return Err(ace(5054, PERMISSIONS, detail, table));
                    }
                    let relative = object.text(dd::link::RELATIVE_PATH).unwrap_or_default();
                    let options = object.info1.unwrap_or(0) as u32;
                    let password = match options & AUTH_ACTIVE_USER {
                        0 => password(db, object)?,
                        _ => Vec::new(),
                    };
                    (
                        relative,
                        object.text(dd::link::USER_NAME),
                        password,
                        options,
                    )
                }
                // A dictionary's path, logged into as the active user.
                None => (link.to_owned(), None, Vec::new(), AUTH_ACTIVE_USER),
            },
        };
        let (user, password) = match options & AUTH_ACTIVE_USER {
            0 => (user, password),
            _ => active(db),
        };
        Ok((resolve(db.folder(), &relative), user, password))
    })?;
    let (path, user, password) = found;
    if let Some(shares) = &session.shares
        && !shares.contains(&path)
    {
        return Err(Error::FileNotOnServer);
    }
    let user = user.filter(|u| !u.is_empty());
    Ok(Target {
        path,
        user,
        password,
    })
}

/// Whether the connection's user may use the link `name` of `db`: always,
/// unless the dictionary verifies access rights on the user, who then needs
/// the link's `ACCESS` right.
fn may_use(db: &Database, name: &str) -> bool {
    use crate::system::right;
    let Some(user) = db.user().filter(|_| db.enforces()) else {
        return true;
    };
    crate::system::permissions_mask(db, user, 12, name, "", true)
        .is_ok_and(|mask| mask & right::ACCESS != 0)
}

/// The password of the link `object` of `db`, which the dictionary keeps
/// wrapped with its key; not readable without the key.
fn password(db: &Database, object: &dd::Object) -> Result<Vec<u8>, Error> {
    let Some(wrapped) = object.slot(dd::link::PASSWORD) else {
        return Ok(Vec::new());
    };
    let key = db.key().ok_or(Error::NotImplemented)?;
    let mut password = wrapped.to_vec();
    tflip_storage::crypto::wrap(db.scheme(), &key, &mut password);
    let end = password
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(password.len());
    password.truncate(end);
    Ok(password)
}

/// The dictionary `target` is, logged into as its user; the error the
/// original engine gives if that fails, for the link `link` and the table
/// `table`.
fn login(link: &str, table: &str, target: &Target) -> Result<Database, Error> {
    let failed = |code: u32, text: &str| {
        let detail = format!("{}: Error {code}:  {text}", shortened(link));
        ace(
            5174,
            "Advantage failed to open the specified link.",
            &detail,
            table,
        )
    };
    let shown = target.path.display();
    if !target.path.is_file() {
        let text = format!(
            "File not found.  Verify the specified path and file name is correct.  {shown}"
        );
        return Err(failed(7041, &text));
    }
    let mut database = match Database::open(&target.path) {
        Ok(database) => database,
        Err(Error::DictionaryUnreadable) => {
            let text = format!(
                "The specified table, memo file, or index file was unable to be opened.  {shown}"
            );
            return Err(failed(7008, &text));
        }
        Err(error) => return Err(error),
    };
    let user = target.user.as_deref();
    let key = match database.login(user, &target.password) {
        Ok(key) => key,
        Err(_) => {
            return Err(match user {
                None => ace(
                    5054,
                    PERMISSIONS,
                    "Anonymous user access is not allowed.",
                    table,
                ),
                Some(user) if !database.has_user(user) => failed(
                    5100,
                    "The given user name is not valid. It may be too long or of length 0.",
                ),
                Some(_) => failed(5097, "The password given is not valid."),
            });
        }
    };
    database.set_identity(user, key);
    database.set_password(&target.password);
    Ok(database)
}

/// What [`Database::login`] refuses a link's or link creation's login with:
/// the original engine's error for no such user (5100), for an anonymous login
/// the dictionary does not allow (5054), or `None` when only the password is
/// wrong — which creating a link does not check.
pub(crate) fn creation_refusal(database: &Database, user: Option<&str>) -> Option<Error> {
    match user.filter(|u| !u.is_empty()) {
        Some(user) if !database.has_user(user) => Some(Error::Sql(SqlError::ace(
            5100,
            "The given user name is not valid. It may be too long or of length 0.",
            "",
        ))),
        Some(_) => None,
        None => database.login(None, b"").err().map(|_| {
            Error::Sql(SqlError::ace(
                5054,
                PERMISSIONS,
                "Anonymous user access is not allowed.",
            ))
        }),
    }
}

/// The original engine's description of a refusal for lack of rights.
const PERMISSIONS: &str = "The command cannot be completed with the current user permissions.";

/// The error `code` as a statement reports it about the table `table`.
fn ace(code: u32, description: &str, detail: &str, table: &str) -> Error {
    Error::Sql(SqlError::ace(
        code,
        description,
        &format!("{detail} Table name: {table}"),
    ))
}

/// A link's name as the original engine shows it in an error: its first 97
/// characters and `...` if it is longer than 100.
fn shortened(link: &str) -> String {
    match link.chars().count() > 100 {
        true => format!("{}...", link.chars().take(97).collect::<String>()),
        false => link.to_owned(),
    }
}

/// `path`, a dictionary's path as a link keeps it (relative to `folder`, the
/// linking dictionary's, unless absolute; either separator), as a path with
/// its `.` and `..` parts worked out.
pub(crate) fn resolve(folder: &Path, path: &str) -> PathBuf {
    let path = path.replace('\\', "/");
    let given = Path::new(&path);
    let joined = match given.is_absolute() {
        true => given.to_path_buf(),
        false => folder.join(given),
    };
    let mut resolved = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other),
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_resolve_from_the_linking_dictionarys_folder() {
        let folder = Path::new("/data/props");
        assert_eq!(
            resolve(folder, r"..\dict\dict.add"),
            Path::new("/data/dict/dict.add")
        );
        assert_eq!(
            resolve(folder, r".\props.add"),
            Path::new("/data/props/props.add")
        );
        assert_eq!(resolve(folder, "nolink"), Path::new("/data/props/nolink"));
        assert_eq!(
            resolve(folder, "/elsewhere/x.add"),
            Path::new("/elsewhere/x.add")
        );
    }

    #[test]
    fn long_link_names_are_shortened_as_the_original_shows_them() {
        let hundred = "a".repeat(100);
        assert_eq!(shortened(&hundred), hundred);
        assert_eq!(
            shortened(&format!("{hundred}b")),
            format!("{}...", "a".repeat(97))
        );
    }
}
