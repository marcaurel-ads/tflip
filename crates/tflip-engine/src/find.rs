//! Table listings: the files of a connection's directory matching a mask.
//!
//! Matching follows what clients see from the original engine: names are
//! bare file names as stored, extension included, in the file system's
//! order; only regular files count. Unlike the original engine, which
//! resolves a relative mask against the process's current directory, a
//! relative mask here is taken relative to the connection's folder (a data
//! dictionary's). On a connection to a data dictionary, an empty mask lists
//! its tables and views, as the original engine does without one.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::Path;

use crate::dictionary::ObjectType;
use crate::handle::{Handle, Listing, Object};
use crate::{Error, Session, connection};

/// The first match of a new table listing.
#[derive(Debug)]
pub struct FirstTable {
    /// The file name as stored, extension included.
    pub name: OsString,
    /// The handle the remaining matches are kept under for
    /// [`find_next_table`].
    pub find: Handle,
    /// Whether it lists a data dictionary's tables.
    pub dictionary: bool,
}

/// Starts a table listing for `mask` in `connection`'s directory.
///
/// Returns the first match and keeps the rest under a new find handle, owned
/// by the connection: disconnecting closes it. Without a match (`Ok(None)`)
/// no listing is opened. With `links` (`AdsFindFirstTable62`), a listing of a
/// dictionary's tables also lists the tables of its links, each as
/// `:link::table` (see `crate::link`).
pub fn find_first_table(
    session: &Session,
    connection: Handle,
    mask: &str,
    links: bool,
) -> Result<Option<FirstTable>, Error> {
    let owner = connection::get(session, connection)?;
    let dictionary = owner.has_database() && mask.is_empty();
    let mut names = match owner.has_database() && mask.is_empty() {
        true if links => crate::link::listing(session, &owner)?
            .into_iter()
            .map(|(name, link)| OsString::from(qualified(name, link)))
            .collect(),
        true => {
            let names = owner
                .with_database_opt(|db| {
                    db.objects(ObjectType::ViewOrTable, None)
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            names.into_iter().map(OsString::from).collect()
        }
        false => {
            let (dir, pattern) = split_mask(mask);
            let folder = owner.folder();
            let dir = match &session.shares {
                Some(shares) => shares.inside(&folder, dir)?,
                None => crate::settings::local_join(session, &folder, dir),
            };
            VecDeque::from(list_tables(&dir, pattern))
        }
    };
    let Some(name) = names.pop_front() else {
        return Ok(None);
    };

    let find = session.insert(
        Object::Find(Listing { names, dictionary }),
        Some(connection),
    );
    Ok(Some(FirstTable {
        name,
        find,
        dictionary,
    }))
}

/// Starts a listing of the objects of type `kind` in `connection`'s data
/// dictionary, under `parent` (a table) if given
/// (`AdsDDFindFirstObject`): the first name and the handle the others are
/// kept under, for [`find_next_table`] and [`find_close`].
pub fn find_first_object(
    session: &Session,
    connection: Handle,
    kind: ObjectType,
    parent: Option<&str>,
) -> Result<(String, Handle), Error> {
    let owner = connection::get(session, connection)?;
    let names = match kind {
        ObjectType::TableViewOrLink => {
            let names: Vec<String> = crate::link::listing(session, &owner)?
                .into_iter()
                .map(|(name, link)| qualified(name, link))
                .collect();
            if names.is_empty() {
                return Err(Error::NoObjectFound);
            }
            names
        }
        _ => owner.with_database(|db| db.objects(kind, parent))?,
    };
    let mut names: VecDeque<OsString> = names.into_iter().map(OsString::from).collect();
    let first = names.pop_front().ok_or(Error::NoObjectFound)?;
    let find = session.insert(
        Object::Find(Listing {
            names,
            dictionary: false,
        }),
        Some(connection),
    );
    Ok((first.to_string_lossy().into_owned(), find))
}

/// A listed table's name: its own, or `:link::table` for a link's.
fn qualified(name: String, link: Option<String>) -> String {
    match link {
        Some(link) => format!(":{link}::{name}"),
        None => name,
    }
}

/// The next name of the listing `find`, or `Ok(None)` after the last one,
/// and whether the listing lists a data dictionary's tables.
pub fn find_next_table(session: &Session, find: Handle) -> Result<Option<(OsString, bool)>, Error> {
    session
        .with(find, |object| match object {
            Object::Find(listing) => Some(
                listing
                    .names
                    .pop_front()
                    .map(|name| (name, listing.dictionary)),
            ),
            _ => None,
        })
        .flatten()
        .ok_or(Error::InvalidHandle)
}

/// Ends the listing `find`. Unknown or already closed handles, and handles
/// that are not listings, are ignored.
pub fn find_close(session: &Session, find: Handle) {
    session.remove_if(find, |object| matches!(object, Object::Find(_)));
}

/// Splits a mask after its last path separator into directory and pattern:
/// `sub\*.adt` → (`sub\`, `*.adt`), `*.adt` → (``, `*.adt`).
fn split_mask(mask: &str) -> (&str, &str) {
    let cut = mask.rfind(is_separator).map_or(0, |i| i + 1);
    mask.split_at(cut)
}

fn is_separator(c: char) -> bool {
    c == '\\' || c == '/'
}

/// The regular files in `dir` whose names match `mask`, in the order NTFS
/// lists a directory, where the original engine runs: by the names'
/// upper-cased UTF-16 code units. The file system's own order would differ
/// from one machine, and one copy of a folder, to the next. Names that are
/// not valid Unicode cannot match and are left out.
fn list_tables(dir: &Path, mask: &str) -> Vec<OsString> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<OsString> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .map(|entry| entry.file_name())
        .filter(|name| name.to_str().is_some_and(|name| mask_matches(mask, name)))
        .collect();
    names.sort_by_cached_key(|name| ntfs_order(name.to_str().unwrap_or_default()));
    names
}

/// The key NTFS orders directory entries by: each UTF-16 code unit
/// upper-cased on its own (a character whose upper case is not a single
/// code unit stays as it is).
fn ntfs_order(name: &str) -> Vec<u16> {
    name.encode_utf16()
        .map(|unit| {
            let Some(c) = char::from_u32(u32::from(unit)) else {
                return unit; // half of a surrogate pair
            };
            let mut upper = c.to_uppercase();
            match (upper.next(), upper.next()) {
                (Some(u), None) if u.len_utf16() == 1 => u as u16,
                _ => unit,
            }
        })
        .collect()
}

/// Whether `name` matches `mask`, where `*` matches any run of characters and
/// `?` any single character. ASCII letters match regardless of case, as
/// Windows file names do.
fn mask_matches(mask: &str, name: &str) -> bool {
    let mask: Vec<char> = mask.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut m, mut n) = (0, 0);
    // After a `*`: the mask position after it and the name position it has
    // swallowed up to, for backtracking.
    let mut star = None;
    while n < name.len() {
        if m < mask.len() && (mask[m] == '?' || mask[m].eq_ignore_ascii_case(&name[n])) {
            m += 1;
            n += 1;
        } else if m < mask.len() && mask[m] == '*' {
            star = Some((m + 1, n));
            m += 1;
        } else if let Some((sm, sn)) = star {
            m = sm;
            n = sn + 1;
            star = Some((sm, sn + 1));
        } else {
            return false;
        }
    }
    mask[m..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// The directory a test listing runs in, created with the given files and
    /// directories (names ending in `/`); removed again on drop.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(tag: &str, entries: &[&str]) -> Self {
            let dir = tflip_storage::testing::temp_root()
                .join(format!("tflip-{tag}-{}", std::process::id()));
            for entry in entries {
                match entry.strip_suffix('/') {
                    Some(sub) => std::fs::create_dir_all(dir.join(sub)).unwrap(),
                    None => {
                        std::fs::create_dir_all(&dir).unwrap();
                        std::fs::write(dir.join(entry), b"").unwrap();
                    }
                }
            }
            TestDir(dir)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn split_mask_cuts_after_last_separator() {
        assert_eq!(split_mask(r"sub\*.adt"), (r"sub\", "*.adt"));
        assert_eq!(split_mask(r"a/b\c*.adt"), (r"a/b\", "c*.adt"));
        assert_eq!(split_mask(r"\*.adt"), (r"\", "*.adt"));
        assert_eq!(split_mask("*.adt"), ("", "*.adt"));
    }

    #[test]
    fn masks() {
        assert!(mask_matches("*.adt", "animals.adt"));
        assert!(mask_matches("*.adt", "ANIMALS.ADT"));
        assert!(!mask_matches("*.adt", "animals.adi"));
        assert!(!mask_matches("*.adt", "animals.adtx"));
        assert!(mask_matches("a?i*.a*", "animals.adt"));
        assert!(mask_matches("*", "x"));
        assert!(mask_matches("*a*b", "xaxab"));
        assert!(!mask_matches("?", ""));
        assert!(mask_matches("?bersicht.adt", "Übersicht.adt"));
    }

    /// Listings come in NTFS's order, whatever order the files were made in:
    /// upper-cased, so case does not matter and `_` (0x5F) follows the
    /// letters, as it would not in a lower-cased order.
    #[test]
    fn listing_is_in_ntfs_order() {
        let dir = TestDir::new(
            "engine-find-order",
            &[
                "z.adt", "_x.adt", "B.adt", "a_.adt", "ab.adt", "a.adt", "A1.adt",
            ],
        );
        assert_eq!(
            list_tables(&dir.0, "*.adt"),
            [
                "a.adt", "A1.adt", "ab.adt", "a_.adt", "B.adt", "z.adt", "_x.adt"
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn listing() {
        let session = Session::new();
        let dir = TestDir::new(
            "engine-find",
            &["sub.adt/", "b.ADT", "a.adt", "a.adi", "c.adm"],
        );
        let connection =
            connection::connect(&session, &format!("Data Source={};", dir.0.display())).unwrap();

        let first = find_first_table(&session, connection, "*.adt", false)
            .unwrap()
            .unwrap();
        let (second, dictionary) = find_next_table(&session, first.find).unwrap().unwrap();
        assert!(!first.dictionary && !dictionary);
        let mut found = [first.name, second];
        found.sort();
        assert_eq!(found, [OsString::from("a.adt"), OsString::from("b.ADT")]);
        assert_eq!(find_next_table(&session, first.find), Ok(None));
        find_close(&session, first.find);
        assert_eq!(
            find_next_table(&session, first.find),
            Err(Error::InvalidHandle)
        );

        assert!(
            find_first_table(&session, connection, "*.dbf", false)
                .unwrap()
                .is_none()
        );
        assert!(
            find_first_table(&session, connection, "", false)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            find_first_table(&session, 0, "*.adt", false).unwrap_err(),
            Error::InvalidConnection
        );

        // Closing a listing never closes its connection.
        find_close(&session, connection);
        assert!(connection::get(&session, connection).is_ok());

        // Disconnecting closes the connection's open listings.
        let open = find_first_table(&session, connection, "*.adt", false)
            .unwrap()
            .unwrap();
        connection::disconnect(&session, connection).unwrap();
        assert_eq!(
            find_next_table(&session, open.find),
            Err(Error::InvalidHandle)
        );
    }
}
