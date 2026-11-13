//! Handles: one table per session for every kind of object handed out to
//! clients.
//!
//! Handle values count up and are never reused, so a handle used after it
//! was closed is reliably unknown rather than reaching a newer object. (The
//! original engine reuses values; clients cannot rely on either.)

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::connection::Connection;
use crate::index::Index;
use crate::statement::Statement;
use crate::table::Table;
use crate::{Error, Session, lock};

/// A handle to an engine object, as handed out to clients.
pub type Handle = usize;

/// What a handle refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandleKind {
    Connection,
    Statement,
    Table,
    /// A table an SQL statement returned.
    Cursor,
    Index,
}

/// An object behind a handle.
pub(crate) enum Object {
    Connection(Arc<Connection>),
    /// A table or object listing.
    Find(Listing),
    Table(Arc<Table>),
    Index(Arc<Index>),
    Statement(Arc<Statement>),
}

/// A listing of tables or dictionary objects.
#[derive(Debug, Default)]
pub(crate) struct Listing {
    /// The names still to be returned.
    pub(crate) names: VecDeque<OsString>,
    /// Whether it lists a data dictionary's tables (a table listing without
    /// a mask).
    pub(crate) dictionary: bool,
}

struct Entry {
    object: Object,
    /// The object this one belongs to (a connection or a table); closed
    /// along with it.
    parent: Option<Handle>,
    /// The value the client keeps on the handle (`AdsSetHandleLong`).
    long: Option<u32>,
}

pub(crate) struct HandleTable {
    entries: BTreeMap<Handle, Entry>,
    next: Handle,
}

impl HandleTable {
    /// An empty table that hands out handles from `base` on.
    pub(crate) fn new(base: Handle) -> Self {
        HandleTable {
            entries: BTreeMap::new(),
            next: base,
        }
    }

    /// Closes `closed` and everything they own, directly or through other
    /// objects.
    fn close(&mut self, mut closed: Vec<Handle>) {
        let mut i = 0;
        while i < closed.len() {
            let parent = closed[i];
            closed.extend(
                self.entries
                    .iter()
                    .filter(|(_, e)| e.parent == Some(parent))
                    .map(|(&h, _)| h),
            );
            i += 1;
        }
        // A table writes what is pending while the index orders it keeps
        // current are still open.
        for handle in &closed {
            if let Some(Entry {
                object: Object::Table(table),
                ..
            }) = self.entries.get(handle)
            {
                table.finish();
            }
        }
        self.entries.retain(|h, _| !closed.contains(h));
    }
}

/// The first handle a session hands out unless it is given another base.
pub const FIRST_HANDLE: Handle = 0x1000;

impl Session {
    /// Hands out a new handle for `object`, owned by `parent` if given.
    pub(crate) fn insert(&self, object: Object, parent: Option<Handle>) -> Handle {
        let mut handles = lock(&self.handles);
        let handle = handles.next;
        handles.next += 1;
        handles.entries.insert(
            handle,
            Entry {
                object,
                parent,
                long: None,
            },
        );
        handle
    }

    /// Keeps `value` on `handle` for the client; [`Error::InvalidHandle`]
    /// if it is not open (or a listing).
    pub fn set_handle_long(&self, handle: Handle, value: u32) -> Result<(), Error> {
        let mut handles = lock(&self.handles);
        match handles.entries.get_mut(&handle) {
            Some(entry) if !matches!(entry.object, Object::Find(_)) => {
                entry.long = Some(value);
                Ok(())
            }
            _ => Err(Error::InvalidHandle),
        }
    }

    /// The value the client keeps on `handle`; [`Error::InvalidHandle`] if
    /// it keeps none or the handle is not open.
    pub fn handle_long(&self, handle: Handle) -> Result<u32, Error> {
        let handles = lock(&self.handles);
        handles
            .entries
            .get(&handle)
            .and_then(|entry| entry.long)
            .ok_or(Error::InvalidHandle)
    }

    /// Calls `f` on the object behind `handle`, or returns `None` for an
    /// unknown handle.
    pub(crate) fn with<R>(&self, handle: Handle, f: impl FnOnce(&mut Object) -> R) -> Option<R> {
        lock(&self.handles)
            .entries
            .get_mut(&handle)
            .map(|entry| f(&mut entry.object))
    }

    /// The handles of the objects `parent` owns directly that `pred` accepts,
    /// in the order they were handed out.
    pub(crate) fn owned(&self, parent: Handle, pred: impl Fn(&Object) -> bool) -> Vec<Handle> {
        let handles = lock(&self.handles);
        handles
            .entries
            .iter()
            .filter(|(_, e)| e.parent == Some(parent) && pred(&e.object))
            .map(|(&h, _)| h)
            .collect()
    }

    /// Calls `f` on every object.
    pub(crate) fn for_each(&self, mut f: impl FnMut(&Object)) {
        lock(&self.handles)
            .entries
            .values()
            .for_each(|entry| f(&entry.object));
    }

    /// Closes `handle` and every object it owns. Returns whether it was open.
    pub(crate) fn remove(&self, handle: Handle) -> bool {
        let mut handles = lock(&self.handles);
        let open = handles.entries.contains_key(&handle);
        if open {
            handles.close(vec![handle]);
        }
        open
    }

    /// Closes `handle` and every object it owns if `pred` accepts its object.
    /// Returns whether it was closed.
    pub(crate) fn remove_if(&self, handle: Handle, pred: impl Fn(&Object) -> bool) -> bool {
        let accepted = self.with(handle, |object| pred(object)).unwrap_or(false);
        accepted && self.remove(handle)
    }

    /// Closes every object `keep` rejects, along with the objects they own.
    pub(crate) fn remove_all(&self, keep: impl Fn(&Object) -> bool) {
        let mut handles = lock(&self.handles);
        let closed: Vec<Handle> = handles
            .entries
            .iter()
            .filter(|(_, e)| !keep(&e.object))
            .map(|(&h, _)| h)
            .collect();
        handles.close(closed);
    }
}

/// What a session has open, as `AdsMgGetActivityInfo` counts it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    /// Open connections.
    pub connections: u32,
    /// Open table handles (work areas).
    pub work_areas: u32,
    /// The distinct table files open.
    pub tables: u32,
    /// The distinct index files open.
    pub indexes: u32,
}

/// What `session` has open.
pub fn activity(session: &Session) -> Activity {
    let mut activity = Activity::default();
    let mut tables = std::collections::BTreeSet::new();
    let mut indexes = std::collections::BTreeSet::new();
    session.for_each(|object| match object {
        Object::Connection(_) => activity.connections += 1,
        Object::Table(table) => {
            activity.work_areas += 1;
            tables.insert(table.path.clone());
        }
        Object::Index(index) => {
            indexes.insert(index.path.clone());
        }
        Object::Find(_) | Object::Statement(_) => {}
    });
    activity.tables = tables.len() as u32;
    activity.indexes = indexes.len() as u32;
    activity
}

/// The number of the session's connections (the users, as
/// `AdsMgGetUserNames` counts them); with `file`, of those with a table of
/// that file open. A relative `file` is taken relative to each connection's
/// folder; names match regardless of case and of `\` or `/`. An empty
/// `file` is [`Error::InvalidPath`], one no connection has open
/// [`Error::FileNotOpen`].
pub fn users(session: &Session, file: Option<&str>) -> Result<u32, Error> {
    let Some(file) = file else {
        return Ok(activity(session).connections);
    };
    if file.is_empty() {
        return Err(Error::InvalidPath);
    }
    let mut connections = std::collections::BTreeSet::new();
    session.for_each(|object| match object {
        Object::Table(table) if is_file(table, file) => {
            connections.insert(table.connection);
        }
        _ => {}
    });
    match connections.len() {
        0 => Err(Error::FileNotOpen),
        n => Ok(n as u32),
    }
}

/// Whether `table`, opened on a connection, is the file `file` names: a
/// relative `file` relative to the table's connection's folder, regardless
/// of case and of `\` or `/`.
fn is_file(table: &Table, file: &str) -> bool {
    let normal = |path: &str| path.replace('\\', "/").to_lowercase();
    let Some(owner) = table.owner() else {
        return false;
    };
    let absolute =
        std::path::Path::new(file).is_absolute() || file.contains(':') || file.starts_with('\\');
    let wanted = match absolute {
        true => normal(file),
        false => normal(&owner.path.join(file).to_string_lossy()),
    };
    normal(&table.path.to_string_lossy()) == wanted
}

/// The index files `session` has open, by their paths, in the order they
/// were opened: of every table handle with `every_handle` (each table's
/// files once per handle), else each file once. With `table`, those of
/// every handle of that table (see [`users`] for how it is named); an empty
/// name is [`Error::InvalidPath`], one no connection has open
/// [`Error::FileNotOpen`].
pub fn open_indexes(
    session: &Session,
    table: Option<&str>,
    every_handle: bool,
) -> Result<Vec<String>, Error> {
    if table == Some("") {
        return Err(Error::InvalidPath);
    }
    let mut open = false;
    let mut files: Vec<(Handle, String)> = Vec::new();
    session.for_each(|object| match object {
        Object::Table(opened) => open |= table.is_some_and(|file| is_file(opened, file)),
        Object::Index(index) => {
            if table.is_some_and(|file| !is_file(index.owner(), file)) {
                return;
            }
            let path = index.path.to_string_lossy().into_owned();
            let per_handle = every_handle || table.is_some();
            let seen = files
                .iter()
                .any(|(handle, seen)| *seen == path && (!per_handle || *handle == index.table));
            if !seen {
                files.push((index.table, path));
            }
        }
        _ => {}
    });
    if table.is_some() && !open {
        return Err(Error::FileNotOpen);
    }
    Ok(files.into_iter().map(|(_, path)| path).collect())
}

/// An open table, as `AdsMgGetOpenTables` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenTable {
    /// The table's file.
    pub path: String,
    pub table_type: crate::table::TableType,
}

/// The tables `session` has open, in the order they were opened: every
/// table handle with `every_handle`, else each file once; then the files
/// its statements hold and its connections keep open for reuse.
pub fn open_tables(session: &Session, every_handle: bool) -> Vec<OpenTable> {
    let mut tables: Vec<OpenTable> = Vec::new();
    session.for_each(|object| {
        let Object::Table(table) = object else { return };
        let path = table.path.to_string_lossy().into_owned();
        if every_handle || tables.iter().all(|open| open.path != path) {
            tables.push(OpenTable {
                path,
                table_type: table.settings.table_type,
            });
        }
    });
    // The files its statements hold after their cursors closed, once each
    // (see `crate::cache::Hold`)...
    for (path, table_type) in crate::statement::held(session) {
        let path = path.to_string_lossy().into_owned();
        if tables.iter().all(|open| open.path != path) {
            tables.push(OpenTable { path, table_type });
        }
    }
    // ...and the files its connections keep open after closing them (see
    // `crate::cache`), which the original engine lists as open too.
    for connection in crate::connection::all(session) {
        for (path, table_type) in crate::cache::kept(&connection) {
            let path = path.to_string_lossy().into_owned();
            if every_handle || tables.iter().all(|open| open.path != path) {
                tables.push(OpenTable { path, table_type });
            }
        }
    }
    tables
}

/// The tables `session` has opened by name ([`crate::table::open`]; not
/// SQL cursors, but views) and still has open, in the order they were
/// opened: those on `connection`, or all for 0.
pub fn tables(session: &Session, connection: Handle) -> Vec<Handle> {
    let handles = lock(&session.handles);
    let opened = |entry: &Entry| match &entry.object {
        Object::Table(table) => {
            table.opened_by_name() && (connection == 0 || table.connection == connection)
        }
        _ => false,
    };
    handles
        .entries
        .iter()
        .filter(|(_, entry)| opened(entry))
        .map(|(&h, _)| h)
        .collect()
}

/// The connections `session` has open, in the order they were opened, with
/// their paths.
pub fn connections(session: &Session) -> Vec<(Handle, String)> {
    let handles = lock(&session.handles);
    let path = |entry: &Entry| match &entry.object {
        Object::Connection(connection) => Some(connection.path.to_string_lossy().into_owned()),
        _ => None,
    };
    handles
        .entries
        .iter()
        .filter_map(|(&h, entry)| Some((h, path(entry)?)))
        .collect()
}

/// What `handle` refers to, or `None` for an unknown handle. Table listings
/// are internal and count as unknown.
pub fn kind(session: &Session, handle: Handle) -> Option<HandleKind> {
    session
        .with(handle, |object| match object {
            Object::Connection(_) => Some(HandleKind::Connection),
            Object::Table(table) if table.statement().is_some() => Some(HandleKind::Cursor),
            Object::Table(_) => Some(HandleKind::Table),
            Object::Index(_) => Some(HandleKind::Index),
            Object::Statement(_) => Some(HandleKind::Statement),
            Object::Find(_) => None,
        })
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection;
    use crate::table::{self, TableSettings, TableType};

    /// A value kept on a handle goes with it, and the handles of the
    /// objects a closed one owned.
    #[test]
    fn handle_longs_go_with_their_handles() {
        let session = Session::new();
        let connection = connection::connect(&session, "Data Source=x;ServerType=LOCAL;").unwrap();
        assert_eq!(session.handle_long(connection), Err(Error::InvalidHandle));
        session.set_handle_long(connection, 42).unwrap();
        assert_eq!(session.handle_long(connection), Ok(42));
        let find = session.insert(Object::Find(Listing::default()), Some(connection));
        assert_eq!(
            session.set_handle_long(find, 1),
            Err(Error::InvalidHandle),
            "a listing"
        );
        connection::disconnect(&session, connection).unwrap();
        assert_eq!(session.handle_long(connection), Err(Error::InvalidHandle));
        assert_eq!(
            session.set_handle_long(connection, 1),
            Err(Error::InvalidHandle)
        );
    }

    #[test]
    fn kinds() {
        let session = Session::new();
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/adt/all_types");
        let source = format!("Data Source={dir};ServerType=LOCAL;");
        let connection = connection::connect(&session, &source).unwrap();
        assert_eq!(kind(&session, connection), Some(HandleKind::Connection));
        let statement = crate::statement::create(&session, connection).unwrap();
        assert_eq!(kind(&session, statement), Some(HandleKind::Statement));
        let settings = TableSettings {
            table_type: TableType::Adt,
            char_type: 1,
            collation: String::new(),
            options: 0,
            ..TableSettings::default()
        };
        let table = table::open(&session, connection, "all_types.adt", settings).unwrap();
        assert_eq!(kind(&session, table), Some(HandleKind::Table));
        assert_eq!(kind(&session, 0), None);
        connection::disconnect(&session, connection).unwrap();
        assert_eq!(kind(&session, connection), None);
    }

    #[test]
    fn closing_a_parent_closes_its_children() {
        let session = Session::new();
        let parent = session.insert(Object::Find(Listing::default()), None);
        let child = session.insert(Object::Find(Listing::default()), Some(parent));
        let grandchild = session.insert(Object::Find(Listing::default()), Some(child));
        let other = session.insert(Object::Find(Listing::default()), None);
        assert!(session.remove(parent));
        assert!(session.with(child, |_| ()).is_none());
        assert!(
            session.with(grandchild, |_| ()).is_none(),
            "owned through the child"
        );
        assert!(session.with(other, |_| ()).is_some());
        assert!(!session.remove(parent));
        assert!(session.remove(other));
    }

    #[test]
    fn handles_are_not_reused() {
        let session = Session::new();
        let first = session.insert(Object::Find(Listing::default()), None);
        session.remove(first);
        let second = session.insert(Object::Find(Listing::default()), None);
        assert_ne!(first, second);
        session.remove(second);
    }

    #[test]
    fn sessions_hand_out_handles_from_their_base() {
        let a = Session::new();
        let b = Session::with_base(0x4000_0000);
        assert_eq!(
            a.insert(Object::Find(Listing::default()), None),
            FIRST_HANDLE
        );
        let handle = b.insert(Object::Find(Listing::default()), None);
        assert_eq!(handle, 0x4000_0000);
        assert!(
            a.with(handle, |_| ()).is_none(),
            "sessions do not share handles"
        );
    }
}
