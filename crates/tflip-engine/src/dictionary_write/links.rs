//! Adding, dropping and modifying links.

use super::*;

/// The linked-dictionary path as the engine stores it: relative to this
/// dictionary's folder, `.\name.add`. A path in the same folder (or a bare
/// filename) becomes `.\<file>`; any other is kept with backslash separators.
fn link_relative_path(add_path: &Path, linked: &str) -> String {
    // Either separator separates (`..\dict\dict.add` is in another folder).
    let linked = linked.replace('\\', "/");
    let linked = Path::new(&linked);
    let folder = dictionary_folder(add_path);
    let file = linked
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or_default();
    let same_folder = linked
        .parent()
        .is_none_or(|p| p.as_os_str().is_empty() || p == Path::new(".") || p == folder);
    if same_folder {
        format!(".\\{file}")
    } else {
        linked.to_string_lossy().replace('/', "\\")
    }
}

/// Builds a link's Property slots: the user name, the `wrap`ped password and the
/// relative path, as the original engine writes them.
fn link_slots<'a>(
    user: &'a [u8],
    password: &'a [u8],
    relative: &'a [u8],
    user_name: &str,
) -> Vec<(usize, &'a [u8])> {
    use tflip_storage::dd::link;
    let mut slots: Vec<(usize, &[u8])> = Vec::new();
    if !user_name.is_empty() {
        slots.push((link::USER_NAME, user));
    }
    // An empty password (its terminator alone) is left out.
    if password.len() > 1 {
        slots.push((link::PASSWORD, password));
    }
    slots.push((link::RELATIVE_PATH, relative));
    slots
}

/// The linked dictionary `linked_path` names (as seen from the dictionary
/// `add_path`) and its opened database, as a link to it needs them: it must be
/// there, open, and know the link's user.
pub(crate) fn open_link_target(
    add_path: &Path,
    linked_path: &str,
    user_name: &str,
    options: u32,
) -> Result<(PathBuf, crate::dictionary::Database), Error> {
    let folder = dictionary_folder(add_path);
    let linked = crate::link::resolve(folder, linked_path);
    let unopened = || ace_error::sql(7008, &linked.display().to_string());
    if !linked.is_file() {
        // No path at all is the folder, with the separator after it.
        let shown = match linked_path.is_empty() {
            true => format!("{}{}", linked.display(), std::path::MAIN_SEPARATOR),
            false => linked.display().to_string(),
        };
        return Err(ace_error::sql(7041, &shown));
    }
    let database = match crate::dictionary::Database::open(&linked) {
        Ok(database) => database,
        Err(Error::DictionaryUnreadable) => return Err(unopened()),
        Err(error) => return Err(error),
    };
    let user = match options & crate::link::AUTH_ACTIVE_USER {
        0 => Some(user_name),
        _ => Some(user_name).filter(|u| !u.is_empty()),
    };
    if let Some(user) = user
        && let Some(refused) = crate::link::creation_refusal(&database, Some(user))
    {
        return Err(refused);
    }
    Ok((linked, database))
}

/// Adds a link to another data dictionary (`AdsDDCreateLink`): a LinkedDD object
/// under the Database, its `Info1` the options, holding the user name, the link
/// password (`wrap`ped with the dictionary key) and the path to the linked
/// dictionary relative to this one's folder — written as the original engine
/// writes it. An empty or already-used alias is refused with 5132; a dictionary
/// whose key tflip cannot recover (neither from the connection's login nor
/// from an empty administrator password) fails as not implemented.
pub fn add_link_api(
    session: &Session,
    connection: Handle,
    alias: &str,
    linked_path: &str,
    user_name: &str,
    password: &str,
    options: u32,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind, create::NewObject};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    if alias.is_empty() {
        return Err(invalid_object_name(alias));
    }
    if options & crate::link::GLOBAL != 0 && !super::rights::may_create(&conn, LINK) {
        return Err(ace_error::sql(
            5054,
            "Cannot create link object in the data dictionary.",
        ));
    }
    let dict = open_dict(&add_path)?;
    // The dictionary's links and the connection's own share their names.
    let local = lock(&conn.links)
        .iter()
        .any(|l| l.name.eq_ignore_ascii_case(alias));
    if local || dict.named(Kind::Link, alias).is_some() {
        return Err(ace_error::sql(5132, "The link alias is already in use."));
    }
    // As in the original engine, the linked dictionary must open and know the
    // link's user, or let it in anonymously, though the password is not
    // checked (probed, `fixtures/sql/links.json`).
    let (linked, database) = open_link_target(&add_path, linked_path, user_name, options)?;
    // Made, a link that logs in is active; a link of the connection's own
    // must log in.
    let login = (user_name, password, options);
    // A link that is not global is the connection's alone, and not written.
    if options & crate::link::GLOBAL == 0 {
        crate::link::activate_made(&conn, alias, &database, &linked, login)?;
        lock(&conn.links).push(crate::link::Local {
            name: alias.to_owned(),
            path: linked_path.to_owned(),
            user: user_name.to_owned(),
            password: password.to_owned(),
            options,
        });
        return Ok(());
    }
    // A static link keeps the dictionary's full path.
    let stored = match options & crate::link::PATH_IS_STATIC {
        0 => link_relative_path(&add_path, linked_path),
        _ => linked.display().to_string(),
    };
    // The key the connection's login recovered, else the one an empty
    // administrator password unlocks.
    let key = super::connection_key(&conn, &dict).ok_or(Error::NotImplemented)?;
    let scheme = dict.scheme();
    drop(dict);

    const DATABASE_ID: i32 = 1;
    let next_id = read_next_id(&add_path)?;
    let user = nul_bytes(user_name);
    let relative = nul_bytes(&stored);
    let mut password = nul_bytes(password);
    tflip_storage::crypto::wrap(scheme, &key, &mut password);
    let slots = link_slots(&user, &password, &relative, user_name);
    let record = dd::create::object_record(&NewObject {
        id: next_id,
        parent: DATABASE_ID,
        kind: "LinkedDD",
        name: alias,
        info1: options as i32,
        info2: 0x8000_0000,
        slots: &slots,
    });
    dd::create::insert_objects(&add_path, &[record], next_id + 1)
        .map_err(|_| Error::CorruptTable)?;
    rebuild_dictionary(session, &conn, &add_path)?;
    super::rights::grant_creator(session, connection, LINK, alias)?;
    crate::link::activate_made(&conn, alias, &database, &linked, login)
}

/// The `ADS_DD_LINK_OBJECT` type, for the rights on a link.
const LINK: u16 = 12;

/// Removes a link from the data dictionary (`AdsDDDropLink`): soft-deletes its
/// LinkedDD object. The alias may be the link's name or its path. An unknown link
/// is refused with 5132. The `drop_global` flag (removing the reverse link in the
/// linked dictionary) is not acted on. A link of the connection's own (not
/// global) is dropped from the connection. Either way the link is no longer
/// active.
pub fn drop_link_api(session: &Session, connection: Handle, alias: &str) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let local = {
        let mut links = lock(&conn.links);
        let before = links.len();
        links.retain(|l| !l.name.eq_ignore_ascii_case(alias));
        links.len() != before
    };
    if local {
        crate::link::deactivate(&conn, alias);
        return Ok(());
    }
    let dict = open_dict(&add_path)?;
    let link = dict
        .all(Kind::Link)
        .into_iter()
        .find(|l| {
            l.name.eq_ignore_ascii_case(alias)
                || l.text(dd::link::RELATIVE_PATH)
                    .is_some_and(|p| p.eq_ignore_ascii_case(alias))
        })
        .ok_or_else(|| no_such_object("link", alias))?;
    let name = link.name.clone();
    // The rights on the link go with it (`fixtures/sql/ddrights`).
    let mut slots = vec![link.record()];
    slots.extend(permission_records(&dict, link.id));
    drop(dict);
    delete_records(session, &conn, &add_path, &slots)?;
    crate::link::deactivate(&conn, &name);
    Ok(())
}

/// Modifies a link to another data dictionary (`AdsDDModifyLink`): rewrites the
/// LinkedDD object's user name, link password (`wrap`ped with the dictionary
/// key), path to the linked dictionary (relative to this one's folder) and
/// `Info1` options, as the original engine would write them. The alias may be
/// the link's name or its stored path. An unknown link is refused with 5132; a
/// dictionary whose key tflip cannot recover (a non-empty administrator
/// password) fails as not implemented.
///
/// The original engine's own `AdsDDModifyLink` cannot act on a link it wrote: it
/// returns 5132 "no such named link" (as its `AdsDDDropLink` and
/// `AdsDDRemoveTrigger` do), so there is no byte-for-byte golden to match — the
/// result is verified by tflip's own round-trip (create, modify, read back).
pub fn modify_link_api(
    session: &Session,
    connection: Handle,
    alias: &str,
    linked_path: &str,
    user_name: &str,
    password: &str,
    options: u32,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};

    let (conn, add_path) = dictionary_connection(session, connection)?;
    let dict = open_dict(&add_path)?;
    let link = dict
        .all(Kind::Link)
        .into_iter()
        .find(|l| {
            l.name.eq_ignore_ascii_case(alias)
                || l.text(dd::link::RELATIVE_PATH)
                    .is_some_and(|p| p.eq_ignore_ascii_case(alias))
        })
        .ok_or_else(|| no_such_object("link", alias))?;
    let record = link.record();
    let key = super::connection_key(&conn, &dict).ok_or(Error::NotImplemented)?;
    let scheme = dict.scheme();
    drop(dict);

    let user = nul_bytes(user_name);
    // A static link keeps the dictionary's full path, as a new one does.
    let stored = match options & crate::link::PATH_IS_STATIC {
        0 => link_relative_path(&add_path, linked_path),
        _ => crate::link::resolve(dictionary_folder(&add_path), linked_path)
            .display()
            .to_string(),
    };
    let relative = nul_bytes(&stored);
    let mut password = nul_bytes(password);
    tflip_storage::crypto::wrap(scheme, &key, &mut password);
    let slots = link_slots(&user, &password, &relative, user_name);
    // Rewrite the property slots and the `Info1` options in one record write
    // (then rebuild). An empty user name clears the slot, so modifying a link
    // back to no user drops its name as the create path would omit it.
    let mut written = Vec::new();
    if user_name.is_empty() {
        written.push((dd::link::USER_NAME, None));
    }
    written.extend(slots.iter().map(|&(index, value)| (index, Some(value))));
    write_object(
        session,
        &conn,
        &add_path,
        record,
        &written,
        Some(options as i32),
    )
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn creates_and_drops_links_like_the_engine() {
        use tflip_storage::dd::{self, Dictionary, Kind};
        let session = Session::new();
        let fx = ops_fixtures();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        // A self-link, as the golden has it (AdsDDCreateLink with an absolute path
        // in the same folder, stored `.\db.add`).
        add_link_api(
            &session,
            connect,
            "mylink",
            &add.to_string_lossy(),
            "ADSSYS",
            "secret",
            1,
        )
        .unwrap();
        for (file, golden) in [
            ("db.add", "linkadd.add"),
            ("db.am", "linkadd.am"),
            ("db.ai", "linkadd.ai"),
        ] {
            let ours = std::fs::read(dir.join(file)).unwrap();
            let want = std::fs::read(fx.join(golden)).unwrap();
            let (ours, want) = if file == "db.add" {
                (mask_user_secrets(ours), mask_user_secrets(want))
            } else {
                (ours, want)
            };
            assert_eq!(ours, want, "{file}");
        }
        // The password tflip wrote unwraps, with the recovered key, to the password
        // and a NUL — the link object's slot 1.
        {
            let dict = Dictionary::open(&add).unwrap();
            let link = dict.named(Kind::Link, "mylink").unwrap();
            assert_eq!(link.info1, Some(1));
            assert_eq!(link.text(dd::link::USER_NAME).as_deref(), Some("ADSSYS"));
            assert_eq!(
                link.text(dd::link::RELATIVE_PATH).as_deref(),
                Some(".\\db.add")
            );
            let key = dict.dictionary_key(b"").unwrap();
            let mut pw = link.slot(dd::link::PASSWORD).unwrap().to_vec();
            tflip_storage::crypto::wrap(dict.scheme(), &key, &mut pw);
            assert_eq!(pw, b"secret\0");
        }
        // A reused or empty alias is refused.
        for alias in ["mylink", "MYLINK", ""] {
            assert!(matches!(
                add_link_api(
                    &session,
                    connect,
                    alias,
                    &add.to_string_lossy(),
                    "x",
                    "y",
                    0
                ),
                Err(Error::Sql(_))
            ));
        }
        // Drop takes the link out (the engine's own AdsDDDropLink fails with 5132,
        // so this is checked for self-consistency).
        drop_link_api(&session, connect, "mylink").unwrap();
        {
            let dict = Dictionary::open(&add).unwrap();
            assert!(dict.named(Kind::Link, "mylink").is_none());
        }
        assert!(matches!(
            drop_link_api(&session, connect, "mylink"),
            Err(Error::Sql(_))
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `AdsDDModifyLink` rewrites a link's path, credentials and options. The
    /// engine's own version fails 5132 on a link it wrote (like `AdsDDDropLink`),
    /// so this is a tflip round-trip: create, modify, read the changes back.
    #[test]
    fn modifies_a_link() {
        use tflip_storage::dd::{self, Dictionary, Kind};
        let session = Session::new();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddlinkmod-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");
        let self_path = add.to_string_lossy().into_owned();
        let connect = create_dictionary_api(&session, &self_path, 0).unwrap();
        add_link_api(
            &session, connect, "mylink", &self_path, "ADSSYS", "secret", 1,
        )
        .unwrap();

        // Rewrite every field: a new user, a new password and new options.
        modify_link_api(
            &session, connect, "mylink", &self_path, "OTHER", "newpass", 4,
        )
        .unwrap();
        {
            let dict = Dictionary::open(&add).unwrap();
            let link = dict.named(Kind::Link, "mylink").unwrap();
            assert_eq!(link.info1, Some(4));
            assert_eq!(link.text(dd::link::USER_NAME).as_deref(), Some("OTHER"));
            // Options 4 make the path static: the dictionary's full path.
            assert_eq!(
                link.text(dd::link::RELATIVE_PATH).as_deref(),
                Some(self_path.as_str())
            );
            let key = dict.dictionary_key(b"").unwrap();
            let mut pw = link.slot(dd::link::PASSWORD).unwrap().to_vec();
            tflip_storage::crypto::wrap(dict.scheme(), &key, &mut pw);
            assert_eq!(pw, b"newpass\0");
        }

        // Modifying to an empty user name clears the slot.
        modify_link_api(&session, connect, "mylink", &self_path, "", "p2", 0).unwrap();
        {
            let dict = Dictionary::open(&add).unwrap();
            let link = dict.named(Kind::Link, "mylink").unwrap();
            assert_eq!(link.info1, Some(0));
            assert_eq!(link.text(dd::link::USER_NAME), None);
        }

        // The link can be found by its stored path too, and an unknown link fails.
        modify_link_api(&session, connect, ".\\db.add", &self_path, "BACK", "p3", 2).unwrap();
        {
            let dict = Dictionary::open(&add).unwrap();
            let link = dict.named(Kind::Link, "mylink").unwrap();
            assert_eq!(link.text(dd::link::USER_NAME).as_deref(), Some("BACK"));
            assert_eq!(link.info1, Some(2));
        }
        assert!(matches!(
            modify_link_api(&session, connect, "nope", &self_path, "x", "y", 0),
            Err(Error::Sql(_))
        ));
        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
