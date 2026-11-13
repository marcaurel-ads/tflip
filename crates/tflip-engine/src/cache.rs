//! Table files a connection keeps open after its handles on them closed
//! (`AdsCacheOpenTables`, `AdsCacheOpenCursors`), so that opening them again
//! is cheap: the file is not opened and read again, it is shared with the
//! cache as with another handle (see [`crate::shared`]).
//!
//! What the original engine does, probed (`tools/ace/cache`): a connection
//! keeps up to N tables it closed (N from `AdsCacheOpenTables`, 0 by
//! default) and the tables of up to N statements it closed (N from
//! `AdsCacheOpenCursors`, 25 by default) open, the oldest closing first.
//! They show as open (`AdsMgGetOpenTables`) but hold no locks; an exclusive
//! open, by any connection, makes them let go of the file, as does dropping
//! the table, which closes every connection's cached tables too.
//! `AdsCloseCachedTables` closes a connection's cached tables (not its
//! cursors' tables), as does `AdsCloseAllTables` for every connection;
//! lowering the tables' N closes what is past it (the cursors' N does not,
//! on the original server); disconnecting closes them all.
//!
//! Before that, a statement holds the tables its `SELECT` read until it
//! closes or runs again, even after its cursor closed ([`Hold`]): they count
//! as open then, so an exclusive open or `DROP TABLE` fails.
//!
//! A server's connections keep them; a client's own process's only when its
//! caching is on (`crate::settings::set_local_caching`), as tflip's local
//! server otherwise keeps none.
//!
//! tflip keeps files, not cursors: a cursor's cache counts the files of the
//! statements' tables, each once. A table opened exclusively is not kept,
//! nor an encrypted one (the file holds its password), nor a table built in
//! memory. A file kept only by caches is reused only if it is still what it
//! was when its last handle closed: one another program changed or replaced
//! is opened afresh.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::connection::Connection;
use crate::lock;
use crate::shared::Shared;
use crate::table::TableType;

/// Which of a connection's two caches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Tables opened by name (`AdsCacheOpenTables`).
    Tables,
    /// The tables SQL statements read (`AdsCacheOpenCursors`).
    Cursors,
}

/// A file a connection keeps open.
struct Entry {
    /// The connection, by its address (see [`key`]).
    connection: usize,
    kind: Kind,
    file: Arc<Shared>,
    /// The table's path and type, as a handle on it had them.
    path: PathBuf,
    table_type: TableType,
}

/// Every connection's kept files, the most recently closed last.
fn entries() -> MutexGuard<'static, Vec<Entry>> {
    static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
    lock(&ENTRIES)
}

/// What tells `connection`'s entries apart: its address, which stays its
/// own while it is open (it lets go of its entries as it closes).
fn key(connection: &Connection) -> usize {
    std::ptr::from_ref(connection) as usize
}

/// Lets go of the files of `closed` (outside the entries' lock).
fn release(closed: Vec<Entry>) {
    for entry in closed {
        entry.file.unpark();
    }
}

/// Takes out of `entries` the oldest of `connection`'s `kind` past `keep`.
fn past(entries: &mut Vec<Entry>, connection: usize, kind: Kind, keep: usize) -> Vec<Entry> {
    let mine = |e: &Entry| e.connection == connection && e.kind == kind;
    let mut count = entries.iter().filter(|e| mine(e)).count();
    let mut closed = Vec::new();
    let mut at = 0;
    while count > keep && at < entries.len() {
        if mine(&entries[at]) {
            closed.push(entries.remove(at));
            count -= 1;
        } else {
            at += 1;
        }
    }
    closed
}

/// Keeps `file`, of the table `path` of type `table_type` a handle of
/// `connection`'s is closing, open in the connection's `kind` cache, if it
/// has room for any; the oldest past its size close. Called while the
/// handle still has the file open.
pub(crate) fn keep(
    connection: &Connection,
    kind: Kind,
    file: &Arc<Shared>,
    path: &Path,
    table_type: TableType,
) {
    let size = usize::from(connection.cache_size(kind));
    if file.path().is_none() {
        return;
    }
    let connection = key(connection);
    let mut entries = entries();
    if size == 0 {
        // The statements' tables a connection keeps stay when it is told to
        // keep none (see `connection::set_cache_size`) until one closes.
        let closed = past(&mut entries, connection, kind, 0);
        drop(entries);
        release(closed);
        return;
    }
    let same =
        |e: &Entry| e.connection == connection && e.kind == kind && Arc::ptr_eq(&e.file, file);
    if let Some(at) = entries.iter().position(same) {
        let entry = entries.remove(at);
        entries.push(entry);
        return;
    }
    file.park();
    let file = Arc::clone(file);
    entries.push(Entry {
        connection,
        kind,
        file,
        path: path.to_owned(),
        table_type,
    });
    let closed = past(&mut entries, connection, kind, size);
    drop(entries);
    release(closed);
}

/// A file an SQL statement holds open, from when it opened the table until
/// it closes or runs again, after its cursor closed too: as with the
/// original engine (probed, `tools/ace/cache`), it counts as open, so
/// neither an exclusive open nor `DROP TABLE` gets it meanwhile. Letting go
/// of it hands it to the connection's statements' cache, as closing a table
/// otherwise does.
pub(crate) struct Hold {
    connection: Arc<Connection>,
    file: Arc<Shared>,
    path: PathBuf,
    table_type: TableType,
    /// Whether the cache may keep it then (see `Table::cacheable`).
    cacheable: bool,
}

impl Hold {
    /// Holds `file`, which a table handle of `connection`'s has open.
    pub(crate) fn new(
        connection: &Arc<Connection>,
        file: Arc<Shared>,
        path: &Path,
        table_type: TableType,
        cacheable: bool,
    ) -> Option<Hold> {
        file.attach(false).ok()?;
        let connection = Arc::clone(connection);
        Some(Hold {
            connection,
            file,
            path: path.to_owned(),
            table_type,
            cacheable,
        })
    }

    /// The held table's path and type.
    pub(crate) fn table(&self) -> (&Path, TableType) {
        (&self.path, self.table_type)
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        if self.cacheable {
            keep(
                &self.connection,
                Kind::Cursors,
                &self.file,
                &self.path,
                self.table_type,
            );
        }
        self.file.detach();
    }
}

/// Whether `session`'s statements hold their tables open until they close
/// ([`Hold`]): a server's do, as the original's; a client's own process's
/// only when its caching is on, so that otherwise a table whose cursor
/// closed is closed.
pub(crate) fn holds(session: &crate::Session) -> bool {
    session.shares.is_some() || crate::settings::local_cache_sizes(session).is_some()
}

/// Closes what `connection` keeps in its `kind` cache past `size`.
pub(crate) fn trim(connection: &Connection, kind: Kind, size: u16) {
    let closed = past(&mut entries(), key(connection), kind, usize::from(size));
    release(closed);
}

/// Closes the files `connection` keeps: those of its `kind` cache, or of
/// both for `None`.
pub(crate) fn close(connection: &Connection, kind: Option<Kind>) {
    let connection = key(connection);
    let closed = take(|e| e.connection == connection && kind.is_none_or(|kind| e.kind == kind));
    release(closed);
}

/// Makes every connection let go of `file` (an exclusive open of it, or a
/// change to it by another program).
pub(crate) fn evict_file(file: &Shared) {
    let closed = take(|e| std::ptr::eq(Arc::as_ptr(&e.file), file));
    release(closed);
}

/// Makes every connection let go of the file at `path` (the table is
/// dropped or deleted).
pub(crate) fn evict_path(path: &Path) {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let closed = take(|e| e.file.path() == Some(&path));
    release(closed);
}

/// Makes every connection let go of the files it keeps in its `kind`
/// cache (`DROP TABLE`).
pub(crate) fn evict_kind(kind: Kind) {
    let closed = take(|e| e.kind == kind);
    release(closed);
}

/// Takes the entries `which` picks out of the cache.
fn take(which: impl Fn(&Entry) -> bool) -> Vec<Entry> {
    let mut entries = entries();
    let (closed, kept): (Vec<Entry>, Vec<Entry>) = entries.drain(..).partition(|e| which(e));
    *entries = kept;
    closed
}

/// The files `connection` keeps open, with their tables' types, oldest
/// first.
pub(crate) fn kept(connection: &Connection) -> Vec<(PathBuf, TableType)> {
    let connection = key(connection);
    entries()
        .iter()
        .filter(|e| e.connection == connection)
        .map(|e| (e.path.clone(), e.table_type))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::Session;
    use crate::connection;
    use crate::handle::Handle;
    use crate::table::{self, TableSettings, TableType, options};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/adt/indexes")
            .join(name)
    }

    fn settings(options: u32) -> TableSettings {
        TableSettings {
            table_type: TableType::Adt,
            char_type: 1,
            collation: "ansi".to_owned(),
            options,
            ..TableSettings::default()
        }
    }

    /// A folder of its own holding `t.adt`/`t.adi`, copies of the fixture
    /// table `name`.
    fn folder(test: &str, name: &str) -> PathBuf {
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-cache-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        put(&dir, name);
        dir
    }

    /// Copies the fixture table `name` over `t` in `dir`, in place.
    fn put(dir: &Path, name: &str) {
        std::fs::copy(fixture(&format!("{name}.adt")), dir.join("t.adt")).unwrap();
        std::fs::copy(fixture(&format!("{name}.adi")), dir.join("t.adi")).unwrap();
    }

    /// The records of `t` as a client opening it on `connection` counts
    /// them; the table is closed again.
    fn count(session: &Session, connection: Handle) -> u32 {
        let handle = table::open_for_client(session, connection, "t.adt", settings(0)).unwrap();
        let count = table::get(session, handle).unwrap().record_count().unwrap();
        table::close(session, handle).unwrap();
        count
    }

    /// A connection to `dir` that keeps up to `cached` tables open.
    fn connect(session: &Session, dir: &Path, cached: u16) -> Handle {
        let source = format!("Data Source={};ServerType=LOCAL;", dir.display());
        let connection = connection::connect(session, &source).unwrap();
        connection::set_cache_size(session, connection, false, cached).unwrap();
        connection
    }

    /// How many tables `connection` keeps open.
    fn kept(session: &Session, connection: Handle) -> usize {
        super::kept(&connection::get(session, connection).unwrap()).len()
    }

    /// A table the connection keeps open is reused only while its file is
    /// what it was: replaced (a new file) or rewritten in place by another
    /// program, it is read afresh.
    #[test]
    fn a_changed_file_is_read_afresh() {
        let fresh = |name: &str| {
            let dir = folder(&format!("fresh-{name}"), name);
            let session = Session::new();
            let connection = connect(&session, &dir, 0);
            count(&session, connection)
        };
        let (indexed, pages) = (fresh("indexed"), fresh("pages8k"));
        assert_ne!(indexed, pages);

        let dir = folder("changed", "indexed");
        let session = Session::new();
        let connection = connect(&session, &dir, 2);
        assert_eq!(count(&session, connection), indexed);
        assert_eq!(kept(&session, connection), 1);
        assert_eq!(count(&session, connection), indexed);
        // Replaced by another file.
        let other = folder("changed-other", "pages8k");
        std::fs::rename(other.join("t.adt"), dir.join("t.adt")).unwrap();
        std::fs::rename(other.join("t.adi"), dir.join("t.adi")).unwrap();
        assert_eq!(count(&session, connection), pages);
        // Rewritten in place.
        put(&dir, "indexed");
        assert_eq!(count(&session, connection), indexed);
        assert_eq!(kept(&session, connection), 1);
        connection::disconnect(&session, connection).unwrap();
    }

    /// A table kept open lets go of it for an exclusive open by another
    /// connection, and closes with `AdsCloseCachedTables` and with its
    /// connection; one opened exclusively is not kept.
    #[test]
    fn kept_tables_yield_and_close() {
        let dir = folder("yield", "indexed");
        let session = Session::new();
        let (first, second) = (connect(&session, &dir, 2), connect(&session, &dir, 2));
        count(&session, first);
        assert_eq!(kept(&session, first), 1);
        let exclusive = settings(options::EXCLUSIVE);
        let handle = table::open_for_client(&session, second, "t.adt", exclusive.clone()).unwrap();
        assert_eq!(kept(&session, first), 0);
        table::close(&session, handle).unwrap();
        assert_eq!(kept(&session, second), 0);
        count(&session, first);
        assert_eq!(kept(&session, first), 1);
        connection::close_cached_tables(&session, first).unwrap();
        assert_eq!(kept(&session, first), 0);
        count(&session, first);
        connection::disconnect(&session, first).unwrap();
        // Its file is closed with it: another opens it exclusively.
        let handle = table::open_for_client(&session, second, "t.adt", exclusive).unwrap();
        table::close(&session, handle).unwrap();
        connection::disconnect(&session, second).unwrap();
    }
}
