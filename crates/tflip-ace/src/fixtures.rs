//! The fixtures written by the original engine, and helpers to use them,
//! for tests.

use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// The process-wide settings (the date format, the server types,
/// whether deleted records show), which tests that change them hold
/// alone ([`settings_lock`]) and tests that need them as they are by
/// default share ([`settings_shared`]).
static SETTINGS: RwLock<()> = RwLock::new(());

/// Serializes the tests that change process-wide settings.
pub fn settings_lock() -> RwLockWriteGuard<'static, ()> {
    SETTINGS.write().unwrap_or_else(PoisonError::into_inner)
}

/// Keeps the process-wide settings from changing while a test needs them
/// as they are by default (the write scenarios).
pub fn settings_shared() -> RwLockReadGuard<'static, ()> {
    SETTINGS.read().unwrap_or_else(PoisonError::into_inner)
}

use std::cell::Cell;
use std::collections::BTreeMap;
use std::net::TcpListener;
use std::sync::{Arc, OnceLock};
use std::thread;

use tflip_engine::Session;
use tflip_engine::shares::Shares;
use tflip_protocol::server;
use tflip_protocol::tls::server::Identity;
use tflip_protocol::wire::Compression;

use crate::{
    ADS_ADT, ADS_LOCAL_SERVER, ADS_REMOTE_SERVER, AE_SUCCESS, AdsConnect60, AdsHandle,
    AdsOpenTable90, Unsigned32,
};

/// The fixture folder `name`: an ADT fixture such as `all_types`, or one
/// of another table type with its type, such as `cdx/dbf_types`.
pub fn dir(name: &str) -> PathBuf {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let dir = match is_xbase(name) {
        true => fixtures.join(name),
        false => fixtures.join("adt").join(name),
    };
    // Named as a connection names it, to compare with what tflip reports.
    PathBuf::from(tflip_storage::universal_path(&dir.to_string_lossy()))
}

/// Whether `name` is a fixture folder of a DBF table type.
fn is_xbase(name: &str) -> bool {
    ["ntx/", "cdx/", "vfp/"]
        .iter()
        .any(|kind| name.starts_with(kind))
}

thread_local! {
    /// Whether the fixtures are reached through a server.
    static REMOTE: Cell<bool> = const { Cell::new(false) };
}

/// Runs `test` with its fixtures on a server ([`server`]): the same
/// exports, through a remote connection.
pub fn remotely(test: fn()) {
    struct Local;
    impl Drop for Local {
        fn drop(&mut self) {
            REMOTE.set(false);
        }
    }
    let _local = Local;
    REMOTE.set(true);
    test();
}

/// Runs `f` as [`remotely`] runs a test, for a closure (a test's own
/// threads).
pub fn remotely_with(f: impl FnOnce()) {
    struct Local;
    impl Drop for Local {
        fn drop(&mut self) {
            REMOTE.set(false);
        }
    }
    let _local = Local;
    REMOTE.set(true);
    f();
}

/// Whether the test runs against a server (see [`remotely`]).
pub fn is_remote() -> bool {
    REMOTE.get()
}

/// A `tflipd` in the test process, serving the fixtures as the share
/// `fixtures`, and trusted by the client: its port.
pub fn server() -> u16 {
    if let Some(port) = SETTINGS_PORT.get() {
        return port;
    }
    static PORT: OnceLock<u16> = OnceLock::new();
    *PORT.get_or_init(|| start_server(false, false))
}

thread_local! {
    /// The port of the server with settings the test runs on
    /// ([`remotely_with_settings`]), if it does.
    static SETTINGS_PORT: Cell<Option<u16>> = const { Cell::new(None) };
}

/// Runs `f` [`remotely`] on a server started with `Check_Free_Table_Access`
/// as `check_free_tables` and `Disable_Free_Connections` as `disable_free`
/// (they bind ADSSYS too), serving what [`server`] does.
pub fn remotely_with_settings(check_free_tables: bool, disable_free: bool, f: impl FnOnce()) {
    static PORTS: Mutex<BTreeMap<(bool, bool), u16>> = Mutex::new(BTreeMap::new());
    let port = *PORTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry((check_free_tables, disable_free))
        .or_insert_with(|| start_server(check_free_tables, disable_free));
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            SETTINGS_PORT.set(None);
        }
    }
    let _reset = Reset;
    SETTINGS_PORT.set(Some(port));
    remotely_with(f);
}

fn start_server(check_free_tables: bool, disable_free: bool) -> u16 {
    {
        let identity = Identity::generate(&["localhost"]).unwrap();
        crate::config::TEST_FINGERPRINTS
            .lock()
            .unwrap()
            .push(identity.fingerprint().unwrap());
        let config = identity.server_config().unwrap();
        let mut shares = Shares::new();
        shares.add("fixtures", dir(""));
        shares.add("xbase", dir("").join(".."));
        shares.add("sqldata", sql_data());
        shares.add("sqldml", sql_dml_data(true));
        shares.add("scratch", scratch_root());
        shares.set_check_free_table_access(check_free_tables);
        shares.set_disable_free_connections(disable_free);
        let shares = Arc::new(shares);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for socket in listener.incoming().flatten() {
                let (config, shares) = (config.clone(), shares.clone());
                thread::spawn(move || {
                    let Ok(mut stream) = server::accept_tls(config, socket) else {
                        return;
                    };
                    // It compresses every client's connection that does not
                    // refuse it, so that remote tests run compressed.
                    let greeting = server::greet(&mut stream, "test tflipd", Compression::Always);
                    let Ok(greeting) = greeting else { return };
                    let base = greeting.handle_base;
                    let session = Session::serving(base, shares);
                    let compressed = greeting.compressed;
                    let _ = server::serve(&mut stream, compressed, &session, |request| {
                        let name = format!("{request:?}");
                        let name = name.split([' ', '(', '{']).next().unwrap_or("").to_owned();
                        lock_requests().entry(base).or_default().push(name);
                    });
                });
            }
        });
        port
    }
}

/// The requests the test server has had from each remote connection,
/// by the first handle of its range.
static REQUESTS: Mutex<BTreeMap<crate::AdsHandle, Vec<String>>> = Mutex::new(BTreeMap::new());

fn lock_requests() -> MutexGuard<'static, BTreeMap<crate::AdsHandle, Vec<String>>> {
    REQUESTS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Held by the tests that count a connection's requests, and by those that
/// call an export that asks every server (`AdsGetAllTables`,
/// `AdsGetNumOpenTables`), whose requests would otherwise land in a count
/// taken at the same time.
pub fn counting() -> MutexGuard<'static, ()> {
    static COUNTING: Mutex<()> = Mutex::new(());
    COUNTING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The first handle of the range of the remote connection `handle`
/// belongs to.
fn range_of(handle: crate::AdsHandle) -> crate::AdsHandle {
    use crate::backend::{REMOTE_BASE, REMOTE_RANGE};
    handle - (handle - REMOTE_BASE) % REMOTE_RANGE
}

/// The requests the process-wide exports send to every remote connection
/// (`AdsGetAllTables`, `AdsSetDateFormat`, `AdsWriteAllRecords`, ...),
/// which tests running alongside make too, setting the date format or the
/// epoch most of all: they land in the count of a connection that is not
/// theirs.
const BROADCASTS: &[&str] = &[
    "tables",
    "close_cached_tables",
    "set_cache_size",
    "set_date_format",
    "set_exact",
    "set_epoch",
    "set_show_deleted",
    "write_all_records",
    "in_transaction",
    "recover_transactions",
];

/// The names of the requests the remote connection of `handle` has sent
/// to the test server since [`forget_requests`], in order.
///
/// Leaves out [`BROADCASTS`]: the process-wide calls of tests running
/// alongside ask them of every remote session, this one's too, and none of
/// the tests that count requests wants one.
pub fn requests(handle: crate::AdsHandle) -> Vec<String> {
    let mut requests = lock_requests()
        .get(&range_of(handle))
        .cloned()
        .unwrap_or_default();
    requests.retain(|name| !BROADCASTS.contains(&name.as_str()));
    requests
}

/// Forgets the requests of the remote connection of `handle`.
pub fn forget_requests(handle: crate::AdsHandle) {
    lock_requests().remove(&range_of(handle));
}

/// A folder of copies of the tables the SQL probes ran on
/// (`fixtures/sql/statements.json` and `select.json`): `all_types`,
/// `keys`, `indexed`, `pages8k` and `fixtures/sql/tables/`, made once
/// per test run.
pub fn sql_data() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let data =
            tflip_storage::testing::temp_root().join(format!("tflip-sql-{}", std::process::id()));
        std::fs::create_dir_all(&data).unwrap();
        for (folder, files) in [
            ("all_types", &["all_types.adt", "all_types.adm"][..]),
            ("adi_keys", &["keys.adt", "keys.adi"]),
            (
                "indexes",
                &["indexed.adt", "indexed.adi", "pages8k.adt", "pages8k.adi"],
            ),
        ] {
            for file in files {
                std::fs::copy(dir(folder).join(file), data.join(file)).unwrap();
            }
        }
        copy_files(&dir("../sql/tables"), &data);
        // The data dictionaries the script connects to, each in its own
        // subfolder, with all their files (as `tools/ace/sql/run.sh`); the
        // props one twice, once in `views/` for the script that changes it.
        for (folder, into) in [
            ("dictionary_props", "props"),
            ("dictionary", "dict"),
            ("dictionary_props", "views"),
        ] {
            copy_files(&dir(folder), &data.join(into));
        }
        data
    })
    .clone()
}

/// A folder for the SQL probe that changes tables
/// (`fixtures/sql/dml.json`): the tables of [`sql_data`] and
/// `fixtures/sql/dml/`, made once per test run for the local replay and
/// once for the remote one.
pub fn sql_dml_data(remote: bool) -> PathBuf {
    static LOCAL: OnceLock<PathBuf> = OnceLock::new();
    static REMOTE: OnceLock<PathBuf> = OnceLock::new();
    let lock = if remote { &REMOTE } else { &LOCAL };
    lock.get_or_init(|| {
        let which = if remote { "remote" } else { "local" };
        let data = tflip_storage::testing::temp_root()
            .join(format!("tflip-sqldml-{}-{which}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        std::fs::create_dir_all(&data).unwrap();
        for folder in [sql_data(), dir("../sql/dml")] {
            copy_files(&folder, &data);
        }
        data
    })
    .clone()
}

/// A connection to [`sql_dml_data`], on the server when the test runs
/// [`remotely`].
pub fn connect_sql_dml() -> AdsHandle {
    let source = match is_remote() {
        true => format!(r"\\127.0.0.1:{}\sqldml", server()),
        false => sql_dml_data(false).display().to_string(),
    };
    connect_to(&source)
}

/// A connection to [`sql_data`], on the server when the test runs
/// [`remotely`].
pub fn connect_sql() -> AdsHandle {
    let source = match is_remote() {
        true => format!(r"\\127.0.0.1:{}\sqldata", server()),
        false => sql_data().display().to_string(),
    };
    connect_to(&source)
}

/// The path a test connects to for the fixture folder `name`: the folder,
/// or its server path when the test runs [`remotely`].
pub fn source(name: &str) -> String {
    match is_remote() {
        true if is_xbase(name) => {
            format!(
                r"\\127.0.0.1:{}\xbase\{}",
                server(),
                name.replace('/', r"\")
            )
        }
        true => format!(r"\\127.0.0.1:{}\fixtures\{name}", server()),
        false => dir(name).display().to_string(),
    }
}

/// A connection to the fixture folder `name`.
pub fn connect(name: &str) -> AdsHandle {
    connect_to(&source(name))
}

/// A connection to the folder `source` (a path, or a server path).
pub fn connect_to(source: &str) -> AdsHandle {
    let (rc, connect) = connect_to_as(source, "", "");
    assert_eq!(rc, AE_SUCCESS, "connecting to {source}");
    connect
}

/// As [`connect_to`], as the user `user` with `password` (an empty one is
/// none), without a check of the result: it and the connection, 0 if it
/// failed.
pub fn connect_to_as(source: &str, user: &str, password: &str) -> (Unsigned32, AdsHandle) {
    let types = if is_remote() {
        ADS_REMOTE_SERVER
    } else {
        ADS_LOCAL_SERVER
    };
    let mut path = format!("{source}\0").into_bytes();
    let credential = |text: &str| (!text.is_empty()).then(|| CString::new(text).unwrap());
    let (user, password) = (credential(user), credential(password));
    let pointer = |text: &Option<CString>| {
        text.as_ref()
            .map_or(std::ptr::null_mut(), |text| text.as_ptr().cast_mut().cast())
    };
    let mut connect = 0;
    let rc = unsafe {
        AdsConnect60(
            path.as_mut_ptr(),
            types,
            pointer(&user),
            pointer(&password),
            0,
            &mut connect,
        )
    };
    (rc, connect)
}

/// The folder the tests that need files of their own make them in, which
/// the test server shares as `scratch`.
fn scratch_root() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tflip_storage::testing::temp_root().join("scratch");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    })
    .clone()
}

/// An empty folder, to start a scratch folder from ([`scratch_from`]).
pub fn empty_dir() -> PathBuf {
    let empty =
        tflip_storage::testing::temp_root().join(format!("tflip-empty-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    empty
}

/// Copies the files of the folder `from` into the folder `to`, which it
/// makes if need be.
pub fn copy_files(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}

/// A folder of its own for the test `test`, holding a copy of the files of
/// the fixture folder `fixture`, as fresh on every call: what the test
/// connects to (see [`source`]).
pub fn scratch(test: &str, fixture: &str) -> String {
    scratch_from(test, &dir(fixture)).0
}

/// As [`scratch`], with a copy of the files of the folder `from`: what the
/// test connects to, and the folder.
pub fn scratch_from(test: &str, from: &Path) -> (String, PathBuf) {
    let name = format!("{test}{}", if is_remote() { "-remote" } else { "" });
    let dir = scratch_root().join(&name);
    let _ = std::fs::remove_dir_all(&dir);
    copy_files(from, &dir);
    let source = match is_remote() {
        true => format!(r"\\127.0.0.1:{}\scratch\{name}", server()),
        false => dir.display().to_string(),
    };
    (source, dir)
}

/// Declares a test running each of the tests `names` of the enclosing
/// test module [`remotely`], in a module `remote`.
macro_rules! remote_twins {
    ($($name:ident),* $(,)?) => {
        mod remote {
            $(
                #[test]
                fn $name() {
                    crate::fixtures::remotely(super::$name);
                }
            )*
        }
    };
}
pub(crate) use remote_twins;

/// A connection to the fixture folder `name` and the ADT table `table` in
/// it, opened the way ARC opens tables.
pub fn open(name: &str, table: &CStr) -> (AdsHandle, AdsHandle) {
    open_with(name, table, c"ansi")
}

/// A connection to the data dictionary `dictionary` in the fixture folder
/// `name` and the table `table` of it, opened by its name there.
pub fn open_in_dictionary(name: &str, dictionary: &str, table: &CStr) -> (AdsHandle, AdsHandle) {
    let separator = if is_remote() { r"\" } else { "/" };
    let source = format!("{}{separator}{dictionary}", source(name));
    let (rc, connect) = connect_to_as(&source, "ADSSYS", "adssyspw");
    assert_eq!(rc, AE_SUCCESS, "connecting to {source}");
    let mut handle = 0;
    let rc = unsafe {
        crate::AdsOpenTable(
            connect,
            table.as_ptr().cast_mut().cast(),
            std::ptr::null_mut(),
            crate::ADS_DEFAULT,
            1,
            1,
            2,
            0,
            &mut handle,
        )
    };
    assert_eq!(rc, AE_SUCCESS);
    (connect, handle)
}

/// As [`open`], with the dynamic collation `collation`.
pub fn open_with(name: &str, table: &CStr, collation: &CStr) -> (AdsHandle, AdsHandle) {
    let connect = connect(name);
    let mut handle = 0;
    let collation = collation.as_ptr().cast_mut().cast();
    let table = table.as_ptr().cast_mut().cast();
    let null = std::ptr::null_mut();
    let rc = unsafe {
        AdsOpenTable90(
            connect,
            table,
            null,
            ADS_ADT,
            1,
            1,
            2,
            0,
            collation,
            &mut handle,
        )
    };
    assert_eq!(rc, AE_SUCCESS);
    (connect, handle)
}
