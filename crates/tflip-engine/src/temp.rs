//! Temporary tables (`#name`): tables SQL makes and names with a `#`, which
//! only the connection that made them sees and which go when it closes
//! (`fixtures/sql/temptables.json`).
//!
//! Each is a table of a file of its own, `tim_<n>.tmp` in a folder of the
//! connection's in the system's temporary folder, that
//! [`crate::table::resolve`] finds by the name; the type it was made as (the
//! statement's) is the type it is opened as later. The original engine keeps
//! them in its cache while they fit, and in its temporary folder otherwise.
//!
//! A Clipper and a FoxPro table are one kind of file, and the original lets
//! either open one until a statement has: the type of the first statement
//! that opens a table is its, and the other is refused (7028) from then on
//! (`fixtures/sql/temptables_api2.json`).

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use tflip_sql::SqlError;

use crate::table::TableType;

/// The counter the connections' folders are named after.
static FOLDERS: AtomicU32 = AtomicU32::new(1);

/// The temporary tables of a connection.
#[derive(Debug, Default)]
pub(crate) struct Temps {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    tables: Vec<Entry>,
    folder: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct Entry {
    /// The name without its `#`, in lower case.
    key: String,
    path: PathBuf,
    table_type: TableType,
    /// Whether a statement has opened it, which fixes `table_type`.
    pinned: bool,
}

/// Whether `name` is the name of a temporary table.
pub(crate) fn is_temp(name: &str) -> bool {
    name.starts_with('#')
}

/// The name of the temporary table `name` as the original's messages give
/// it: without the `#`.
pub(crate) fn bare(name: &str) -> &str {
    name.strip_prefix('#').unwrap_or(name)
}

/// 7112: the temporary table `name` does not exist; `table` says so a
/// `SELECT` or `INSERT` names it in the message too.
pub(crate) fn not_found(name: &str, table: bool) -> SqlError {
    let detail = match table {
        true => format!("{} Table name: {name}", bare(name)),
        false => bare(name).to_owned(),
    };
    SqlError::ace(7112, "The temporary table cannot be found.", &detail)
}

/// 7016 or 7028: the temporary table `name`, made as `made`, is read as
/// `asked`. An ADT table, a Visual FoxPro one and the Clipper and FoxPro ones
/// are of three kinds, and one opened as another kind is corrupt (7016); a
/// Clipper table opened as FoxPro and the other way round has an invalid open
/// mode (7028), once a statement has opened it (`pinned`). `None` for the
/// type it was made as. `table` says so a `SELECT`, `INSERT`, `UPDATE`,
/// `DELETE` or `CREATE INDEX` names it in the message too.
pub(crate) fn type_mismatch(
    name: &str,
    made: TableType,
    asked: TableType,
    pinned: bool,
    table: bool,
) -> Option<SqlError> {
    use TableType::{Cdx, Ntx};
    let (code, text) = match (made, asked) {
        (made, asked) if made == asked => return None,
        (Ntx, Cdx) | (Cdx, Ntx) if !pinned => return None,
        (Ntx, Cdx) | (Cdx, Ntx) => (7028, "Invalid open mode."),
        _ => (
            7016,
            "Corrupt table.  Make sure you are not attempting to open a DBF with an ADT table \
             type or vice versa.",
        ),
    };
    let detail = match table {
        true => format!("{} Table name: {name}", bare(name)),
        false => bare(name).to_owned(),
    };
    Some(SqlError::ace(code, text, &detail))
}

thread_local! {
    /// The temporary tables the statement running on the thread read in its
    /// last run, and those it reads in this one.
    static READS: std::cell::RefCell<Option<(Vec<String>, Vec<String>)>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs `f` as a run of a statement whose last run read the temporary tables
/// `before`, and returns what it read in this one.
///
/// The original's statement keeps the tables a `SELECT` opened until it runs
/// the next, which reads them again as they are, whatever the type it is of
/// now: a temporary table made as another type is refused to a statement
/// that did not read it in its last run, a `SELECT`, and to every other kind
/// of statement (`fixtures/sql/temptables_types.json`).
pub(crate) fn reading<R>(before: Vec<String>, f: impl FnOnce() -> R) -> (R, Vec<String>) {
    let outer = READS.replace(Some((before, Vec::new())));
    let result = f();
    let (_, read) = READS.replace(outer).unwrap_or_default();
    (result, read)
}

/// Whether the statement running on the thread read the temporary table
/// `name` in its last run.
pub(crate) fn read_before(name: &str) -> bool {
    let key = Temps::key(name);
    READS.with_borrow(|reads| {
        reads
            .as_ref()
            .is_some_and(|(before, _)| before.contains(&key))
    })
}

/// The statement running on the thread reads the temporary table `name`.
pub(crate) fn note_read(name: &str) {
    let key = Temps::key(name);
    READS.with_borrow_mut(|reads| {
        if let Some((_, read)) = reads
            && !read.contains(&key)
        {
            read.push(key);
        }
    });
}

/// The statement running on the thread opened the temporary table `name` of
/// the connection's `temps` as `asked`, which is the type of the table from
/// now on (a client's open of a handle does not fix it).
pub(crate) fn pin(temps: &Temps, name: &str, asked: TableType) {
    if READS.with_borrow(Option::is_some) {
        temps.pin(name, asked);
    }
}

/// The statement running on the thread did not read the temporary table
/// `name` for its next run to find it held: `SELECT … INTO #name` makes and
/// reads it, and the next statement of another type is refused it all the same
/// (`fixtures/sql/temptables_api3.json`).
pub(crate) fn forget_read(name: &str) {
    let key = Temps::key(name);
    READS.with_borrow_mut(|reads| {
        if let Some((_, read)) = reads {
            read.retain(|k| *k != key);
        }
    });
}

impl Temps {
    fn key(name: &str) -> String {
        bare(name).to_lowercase()
    }

    /// Whether a statement has opened the temporary table `name`.
    pub(crate) fn pinned(&self, name: &str) -> bool {
        let key = Self::key(name);
        let inner = crate::lock(&self.inner);
        inner
            .tables
            .iter()
            .any(|entry| entry.key == key && entry.pinned)
    }

    fn pin(&self, name: &str, asked: TableType) {
        use TableType::{Cdx, Ntx};
        let key = Self::key(name);
        let mut inner = crate::lock(&self.inner);
        if let Some(entry) = inner.tables.iter_mut().find(|entry| entry.key == key)
            && !entry.pinned
        {
            entry.pinned = true;
            if matches!((entry.table_type, asked), (Ntx, Cdx) | (Cdx, Ntx)) {
                entry.table_type = asked;
            }
        }
    }

    /// The type the temporary table `name` opens as for `asked`: a Clipper or
    /// FoxPro table no statement has opened opens as the one asked for.
    pub(crate) fn open_type(&self, name: &str, asked: TableType) -> Option<TableType> {
        use TableType::{Cdx, Ntx};
        let key = Self::key(name);
        let inner = crate::lock(&self.inner);
        let entry = inner.tables.iter().find(|entry| entry.key == key)?;
        Some(match (entry.table_type, asked) {
            (Ntx, Cdx) | (Cdx, Ntx) if !entry.pinned => asked,
            (made, _) => made,
        })
    }

    /// The file and type of the temporary table `name`.
    pub(crate) fn find(&self, name: &str) -> Option<(PathBuf, TableType)> {
        let key = Self::key(name);
        let inner = crate::lock(&self.inner);
        let entry = inner.tables.iter().find(|entry| entry.key == key)?;
        Some((entry.path.clone(), entry.table_type))
    }

    /// Makes room for a new temporary table `name` of `table_type` and
    /// returns the file it is written to: `None` if there is one already.
    pub(crate) fn create(&self, name: &str, table_type: TableType) -> Option<PathBuf> {
        let key = Self::key(name);
        let mut inner = crate::lock(&self.inner);
        // A table whose creation failed has a name and no file.
        if inner
            .tables
            .iter()
            .any(|entry| entry.key == key && entry.path.exists())
        {
            return None;
        }
        inner.tables.retain(|entry| entry.key != key);
        let folder = inner
            .folder
            .get_or_insert_with(|| {
                let n = FOLDERS.fetch_add(1, Ordering::Relaxed);
                process_folder().join(n.to_string())
            })
            .clone();
        let _ = std::fs::create_dir_all(&folder);
        let n = crate::statement::next_temporary();
        let path = folder.join(format!("tim_{n:x}.tmp"));
        inner.tables.push(Entry {
            key,
            path: path.clone(),
            table_type,
            pinned: false,
        });
        Some(path)
    }

    /// Forgets the temporary table `name` and returns its file.
    pub(crate) fn remove(&self, name: &str) -> Option<PathBuf> {
        let key = Self::key(name);
        let mut inner = crate::lock(&self.inner);
        let at = inner.tables.iter().position(|entry| entry.key == key)?;
        Some(inner.tables.remove(at).path)
    }
}

/// `tflip-temp/<pid>` in the temporary folder. A connection takes its
/// folder with it, but one a process held when it ended (killed, or a static
/// that is never dropped) stays: the first connection to make a temporary
/// table removes those of the processes that have ended.
fn process_folder() -> PathBuf {
    static FOLDER: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    FOLDER
        .get_or_init(|| {
            let parent = std::env::temp_dir().join("tflip-temp");
            tflip_storage::process::sweep(&parent, |pid, _| {
                tflip_storage::process::ended(pid) == Some(true)
            });
            parent.join(std::process::id().to_string())
        })
        .clone()
}

impl Drop for Temps {
    /// The tables go with the connection.
    fn drop(&mut self) {
        let inner = crate::lock(&self.inner);
        if let Some(folder) = &inner.folder {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Error, Session, connection, statement};

    fn run(session: &Session, connection: crate::handle::Handle, sql: &str) -> Result<(), Error> {
        let stmt = statement::create(session, connection)?;
        let cursor = statement::execute_direct(session, stmt, sql, true)?;
        if let Some(cursor) = cursor {
            crate::table::close(session, cursor)?;
        }
        statement::close(session, stmt)
    }

    #[test]
    fn a_temporary_table_is_its_connections_and_goes_with_it() {
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-temp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let session = Session::new();
        let connect = || {
            let text = format!("Data Source={};", dir.display());
            connection::connect(&session, &text).unwrap()
        };
        let (first, second) = (connect(), connect());
        run(&session, first, "CREATE TABLE #t (x integer)").unwrap();
        run(&session, first, "INSERT INTO #t VALUES (1)").unwrap();
        run(&session, first, "SELECT * FROM #t").unwrap();
        // Another connection has no such table, and may make one of its own.
        let error = run(&session, second, "SELECT * FROM #t").unwrap_err();
        assert!(
            matches!(&error, Error::Sql(e) if e.native == 7112),
            "{error:?}"
        );
        run(&session, second, "CREATE TABLE #t (y char(3))").unwrap();
        // Nothing lands in the connection's folder.
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
        let owner = connection::get(&session, first).unwrap();
        let (path, _) = owner.temps.find("#T").unwrap();
        assert!(path.exists());
        drop(owner);
        connection::disconnect(&session, first).unwrap();
        assert!(!path.exists());
        run(&session, second, "DROP TABLE #t").unwrap();
        connection::disconnect(&session, second).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
