//! Connections.

use std::collections::BTreeMap;
use std::ffi::{OsStr, c_void};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tflip_engine::connection::{self, ServerType};
use tflip_engine::format::{self, FieldText, Layout};
use tflip_engine::handle::{Handle, HandleKind};
use tflip_engine::{Error, settings};
use tflip_protocol::Api;
use tflip_protocol::wire::Compression;
use tflip_storage::field::FieldType;
use tflip_storage::value::Value;

use crate::backend::{self, Backend, api};
use crate::config;
use crate::ffi::text;
use crate::handle::{answer, connection_check};
use crate::last_error::{Call, add_detail, fail, fail_handle};
use crate::remote::{self, server_path};
use crate::{
    ADS_AIS_SERVER, ADS_LOCAL_SERVER, ADS_REMOTE_SERVER, ADS_TRUE, AE_INVALID_CONNECTION_HANDLE,
    AE_INVALID_HANDLE, AE_INVALID_OPTION, AE_INVALID_POINTER, AE_INVALID_PROPERTY_LENGTH,
    AE_NO_FILE_FOUND, AE_SUCCESS, AdsHandle, LOCAL, Signed32, Unsigned8, Unsigned16, Unsigned32,
    ansi, code, error_code, write_cut_at, write_name, write_string_at,
};

/// Connects to `path` on a server if `server_types` allows a remote one and
/// `path` is a server path (see [`crate::remote`]); falls back to `local`
/// if that fails and the local server is allowed too. Anything else goes to
/// `local`.
fn connect_to(
    path: Option<&str>,
    server_types: u16,
    user: Option<&str>,
    password: Option<&str>,
    connect_string: Option<&str>,
    compression: Option<Compression>,
    local: impl FnOnce() -> Result<Handle, Error>,
) -> Result<Handle, Error> {
    // A library of an external procedure, in `tflipd`, connects as one in the
    // original's server does (`fixtures/sql/aeprulesremote.json`): the server
    // itself only for a type that allows an Internet server (and any asked
    // for by default); the local server cannot be loaded (6082), a remote one
    // is no address (6097).
    let hosted = backend::hosted();
    if hosted && server_types & ADS_AIS_SERVER == 0 {
        return Err(Error::FullText(match server_types & ADS_REMOTE_SERVER {
            0 => 6082,
            _ => 6097,
        }));
    }
    // It never goes to a remote server, and is the server's own connection.
    if hosted {
        let handle = local()?;
        backend::mark_inside(handle);
        return Ok(handle);
    }
    let remote = server_types & (ADS_REMOTE_SERVER | ADS_AIS_SERVER) != 0;
    // As to an Internet server only if a remote server is not allowed, as the
    // original engine tries a remote server first.
    let internet = server_types & ADS_AIS_SERVER != 0 && server_types & ADS_REMOTE_SERVER == 0;
    // Through a mapped drive the path is a server path too.
    let path = path.map(tflip_storage::universal_path);
    let path = path.as_deref();
    match path.and_then(server_path).filter(|_| remote) {
        Some(server) => {
            // A connect string may ask for a transport tflipd cannot give; if
            // so, the connection fails unless the local server is allowed too.
            let result = match connect_string.map(remote::check_comm) {
                Some(Err(error)) => Err(error),
                _ => {
                    let compression = compression.unwrap_or_else(config::compression);
                    let how = remote::How {
                        user,
                        password,
                        dd_password: connect_string.and_then(|string| {
                            connection::connect_string_value(string, "ddpassword")
                        }),
                        encryption_type: connection::encryption_type(connect_string.and_then(
                            |string| connection::connect_string_value(string, "encryptiontype"),
                        )),
                        compression,
                        internet,
                    };
                    remote::connect(&server, path.unwrap_or_default(), &how)
                }
            };
            result.or_else(|error| {
                if server_types & ADS_LOCAL_SERVER != 0 {
                    local()
                } else {
                    Err(error)
                }
            })
        }
        None => local(),
    }
}

/// Writes `result`, a new connection's handle, to `connect`.
fn connected(result: Result<Handle, Error>, connect: &mut AdsHandle) -> Unsigned32 {
    match result {
        Ok(handle) => {
            *connect = handle;
            AE_SUCCESS
        }
        Err(error) => error_code(error),
    }
}

/// `AdsConnect101`: opens a connection described by a connect string such as
/// `Data Source=C:\data\;ServerType=LOCAL;`.
///
/// `Data Source`, `ServerType`, `User ID`, `Password`, `EncryptionType` and
/// `Compression` are read, and the options of [`crate::connect_options`]:
/// as in the original engine, a key it does not know or a value it refuses
/// fails with `AE_INVALID_OPTION`, the value (or key) added to its text, and
/// `TableType`, `CharType`, `ReadOnly`, `Shared` and `LockMode` are what
/// `AdsOpenTable101` opens tables with, `DateFormat`, `Exact` and `Collation`
/// the connection's own. A `Compression` other than `INTERNET`, `ALWAYS` or
/// `NEVER` (in any case) fails whatever the server type; it means nothing
/// to the local server. `connection_options` is not written.
/// Without one, the client's setting in `tflip.ini` applies (see
/// [`config::compression`]), not `ads.ini`'s. For a connection that goes to
/// `tflipd`, the communication settings (`CommType` and the `TLS*`/`FIPS`
/// keys) are read too: a non-TLS `CommType` is refused, the rest are logged
/// and accepted (see [`remote::check_comm`]). A server path with a remote
/// server type goes to `tflipd` (see `crate::remote`); see
/// [`tflip_engine::connection`] for what the local server checks.
///
/// # Safety
///
/// `connect_string` must be null or point to a NUL-terminated string;
/// `connect` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsConnect101(
    connect_string: *mut Unsigned8,
    _connection_options: *mut AdsHandle,
    connect: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsConnect101");
    // SAFETY: `connect_string` is null or NUL-terminated and `connect` null
    // or writable, as # Safety says.
    let (connect_string, connect) = unsafe { (text(connect_string), connect.as_mut()) };
    let Some(connect) = connect else {
        return fail(AE_INVALID_POINTER);
    };
    let connect_string = connect_string.unwrap_or_default();
    let options = match crate::connect_options::parse(&connect_string) {
        Ok(options) => options,
        Err(detail) if detail.is_empty() => return fail(AE_INVALID_OPTION),
        Err(detail) => return add_detail(fail(AE_INVALID_OPTION), &detail),
    };
    let source = connection::connect_string_value(&connect_string, "data source")
        .or_else(|| connection::connect_string_value(&connect_string, "datasource"));
    if source.is_none_or(str::is_empty) {
        return crate::last_error::fail_detail(
            AE_INVALID_OPTION,
            "Data Source must be provided in connection string.",
        );
    }
    // A collation the repository does not have fails the connect, as it fails
    // `AdsSetCollation`.
    let collation = match options
        .collation
        .clone()
        .map(|name| crate::settings::check_collation(name, false))
    {
        Some(Err(code)) => return code,
        Some(Ok(name)) => Some(name),
        None => None,
    };
    let server_type = connection::connect_string_value(&connect_string, "servertype");
    let server_types = server_type.map_or(0, connection::server_type_bits);
    let user = connection::connect_string_value(&connect_string, "user id");
    let password = connection::connect_string_value(&connect_string, "password");
    let compression = match connection::connect_string_value(&connect_string, "compression") {
        Some(value) => match Compression::parse(value) {
            Some(compression) => Some(compression),
            None if value.is_empty() => return fail(AE_INVALID_OPTION),
            None => return add_detail(fail(AE_INVALID_OPTION), value),
        },
        None => None,
    };
    let result = connect_to(
        source,
        server_types,
        user,
        password,
        Some(&connect_string),
        compression,
        || LOCAL.connect(connect_string.clone()),
    );
    if let Ok(handle) = result {
        // The connection's own settings; a date format the original refuses
        // fails the connect.
        let backend = api(handle);
        if let Some(format) = &options.date_format
            && let Err(error) = backend.set_date_format(handle, format.clone())
        {
            let _ = error;
            let _ = backend.disconnect(handle);
            return add_detail(fail(AE_INVALID_OPTION), format);
        }
        if let Some(name) = collation {
            let _ = backend.set_collation(handle, name);
        }
        if let Some(exact) = options.exact {
            let _ = backend.set_exact(handle, exact);
        }
        if options.check_free_tables {
            check_free_tables(handle);
        }
        // The server's backup of free tables opens them as this type.
        if options.table_type != tflip_engine::table::TableType::Adt {
            let _ = backend.set_table_type(handle, options.table_type);
        }
        crate::connect_options::remember(handle, options);
        remember(handle, password.map(str::to_owned));
    }
    connected(result, connect)
}

/// The passwords connections were opened with, which
/// `AdsGetConnectionProperty` reports: kept by the client so it can report them
/// and open encrypted tables. A server connection also forwards the password so
/// the server can authenticate the login (over TLS, as the original engine does).
static PASSWORDS: Mutex<BTreeMap<Handle, String>> = Mutex::new(BTreeMap::new());

fn passwords() -> MutexGuard<'static, BTreeMap<Handle, String>> {
    PASSWORDS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The password connection `handle` was opened with, if any — used to open an
/// encrypted table on it.
pub(crate) fn password_for(handle: Handle) -> Option<String> {
    passwords().get(&handle).cloned()
}

/// Keeps the password connection `handle` was opened with, if any.
fn remember(handle: Handle, password: Option<String>) {
    if let Some(password) = password {
        passwords().insert(handle, password);
    }
}

/// `AdsConnect`: opens a connection to a folder of free tables or a data
/// dictionary (`.add`), on the servers `AdsSetServerType` allows (remote and
/// Internet servers until set). See [`AdsConnect26`].
///
/// # Safety
///
/// As for [`AdsConnect26`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsConnect(
    server_name: *mut Unsigned8,
    connect: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsConnect");
    // SAFETY: `AdsConnect26`'s contract is this one's.
    unsafe { AdsConnect26(server_name, 0, connect) }
}

/// `AdsConnect26`: opens a connection to a folder of free tables or a data
/// dictionary (`.add`) on one of the servers `server_types` allows
/// (`ADS_LOCAL_SERVER | ...`; 0 for those `AdsSetServerType` allows).
///
/// A server path with a remote server type goes to `tflipd` (see
/// `crate::remote`), falling back to the local server if it is allowed
/// too. As in the original engine, without the local server a set with a
/// remote server fails with 6420 (not in `ace.h`), an Internet server alone
/// with `AE_NO_DRIVE_CONNECTION`. See [`tflip_engine::connection`] for what
/// the local server checks.
///
/// # Safety
///
/// `server_name` must be null or point to a NUL-terminated string;
/// `connect` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsConnect26(
    server_name: *mut Unsigned8,
    server_types: Unsigned16,
    connect: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsConnect26");
    // SAFETY: `server_name` is null or NUL-terminated and `connect` null or
    // writable, as # Safety says.
    let (path, connect) = unsafe { (text(server_name), connect.as_mut()) };
    connect_as(path, server_types, None, None, None, connect)
}

/// Opens a connection to `path` as [`AdsConnect26`] does, as `user` if
/// given; `None` for a null pointer.
fn connect_as(
    path: Option<String>,
    server_types: Unsigned16,
    user: Option<String>,
    password: Option<String>,
    compression: Option<Compression>,
    connect: Option<&mut AdsHandle>,
) -> Unsigned32 {
    let (Some(path), Some(connect)) = (path, connect) else {
        return fail(AE_INVALID_POINTER);
    };
    let types = if server_types == 0 {
        settings::server_types(&LOCAL)
    } else {
        server_types
    };
    let (user_name, password_text) = (user.as_deref(), password.as_deref());
    let result = connect_to(
        Some(&path),
        types,
        user_name,
        password_text,
        None,
        compression,
        || {
            // A library's own connection is the server's, whatever the types say.
            let own = types
                | if backend::hosted() {
                    ADS_LOCAL_SERVER
                } else {
                    0
                };
            LOCAL.connect_path(path.clone(), own, user.clone(), password.clone(), None)
        },
    );
    connected(result, connect)
}

/// `AdsDDCreate`: creates the data dictionary at `dictionary_path` (an `.add`
/// file, locally) and connects to it as `ADSSYS`, writing the connection to
/// `dictionary`. A non-zero `encrypt` makes a dictionary that encrypts its
/// records with RC4; `description` is ignored.
///
/// # Safety
///
/// `dictionary_path` and `description` must be null or point to
/// NUL-terminated strings; `dictionary` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDCreate(
    dictionary_path: *mut Unsigned8,
    encrypt: Unsigned16,
    _description: *mut Unsigned8,
    dictionary: *mut AdsHandle,
) -> Unsigned32 {
    let _call = Call::begin("AdsDDCreate");
    // SAFETY: `dictionary_path` is null or NUL-terminated and `dictionary`
    // null or writable, as # Safety says.
    let (path, dictionary) = unsafe { (text(dictionary_path), dictionary.as_mut()) };
    let (Some(path), Some(dictionary)) = (path, dictionary) else {
        return fail(AE_INVALID_POINTER);
    };
    let result = LOCAL.create_dictionary(path, encrypt);
    connected(result, dictionary)
}

/// `AdsDDCreate101`: creates a data dictionary as [`AdsDDCreate`] does, its
/// path the `Data Source` of a connect string such as
/// `Data Source=C:\data\db.add;`. With `EncryptionType=AES128` or `AES256`
/// and a `DDPassword` it is an AES dictionary. The options are ignored.
///
/// # Safety
///
/// `connect_string` must be null or point to a NUL-terminated string;
/// `dictionary` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsDDCreate101(
    connect_string: *mut Unsigned8,
    _connect_options: *mut AdsHandle,
    dictionary: *mut AdsHandle,
) -> Unsigned32 {
    let _call = Call::begin("AdsDDCreate101");
    // SAFETY: `connect_string` is null or NUL-terminated and `dictionary`
    // null or writable, as # Safety says.
    let (connect_string, dictionary) = unsafe { (text(connect_string), dictionary.as_mut()) };
    let Some(dictionary) = dictionary else {
        return fail(AE_INVALID_POINTER);
    };
    let result = LOCAL.create_dictionary_connect(connect_string.unwrap_or_default());
    connected(result, dictionary)
}

/// The `AdsConnect60` options the original engine accepts: `ADS_INC_USERCOUNT`,
/// `ADS_STORED_PROC_CONN`, the `ADS_COMPRESS_*` settings,
/// `ADS_REPLICATION_CONNECTION` (probed, `tools/ace/compress`), the
/// `ADS_*_CONNECTION` protocols, `ADS_TLS_CONNECTION` and
/// `ADS_CHECK_FREE_TABLE_ACCESS`.
const CONNECT_OPTIONS: Unsigned32 =
    0x0000_0001 | 0x0000_0002 | 0x0000_000C | 0x0000_0010 | 0x0000_00E0 | 0x0000_1000 | CHECK_FREE;

/// `ADS_CHECK_FREE_TABLE_ACCESS`: a dictionary connection that may not open
/// free tables, but for its administrators.
const CHECK_FREE: Unsigned32 = 0x0000_2000;

/// `AdsConnect60`: opens a connection as [`AdsConnect26`] does, with a user
/// name, password and options. Options the original engine does not know
/// fail with `AE_INVALID_OPTION`; the others mean nothing to the local
/// server. An `ADS_COMPRESS_*` option is the connection's compression
/// setting for `tflipd` (see [`config::compression`] for none).
///
/// On a data dictionary, as in the original engine, a user that is neither
/// `ADSSYS` nor one of the dictionary's, or no user where the dictionary
/// requires logins, fails with 7078; a wrong password fails as the
/// original's does.
///
/// # Safety
///
/// `server_path`, `user_name` and `password` must be null or point to
/// NUL-terminated strings; `connect` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsConnect60(
    server_path: *mut Unsigned8,
    server_types: Unsigned16,
    user_name: *mut Unsigned8,
    password: *mut Unsigned8,
    options: Unsigned32,
    connect: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsConnect60");
    if options & !CONNECT_OPTIONS != 0 {
        return fail(AE_INVALID_OPTION);
    }
    // SAFETY: the strings are null or NUL-terminated and `connect` null or
    // writable, as # Safety says.
    let (path, user, password, mut connect) = unsafe {
        (
            text(server_path),
            text(user_name),
            text(password),
            connect.as_mut(),
        )
    };
    let compression = Compression::from_options(options);
    let rc = connect_as(
        path,
        server_types,
        user,
        password.clone(),
        compression,
        connect.as_deref_mut(),
    );
    if rc == AE_SUCCESS
        && let Some(connect) = connect
    {
        remember(*connect, password);
        if options & CHECK_FREE != 0 {
            check_free_tables(*connect);
        }
    }
    rc
}

/// Makes the connection `handle` refuse free tables to its users that are
/// no administrators, on the server it is with.
pub(crate) fn check_free_tables(handle: AdsHandle) {
    let _ = api(handle).set_check_free_tables(handle, true);
}

/// `AdsGetConnectionType`: the kind of server behind a connection, as one of
/// the `ADS_*_SERVER` constants.
///
/// # Safety
///
/// `connect_type` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetConnectionType(
    connect: AdsHandle,
    connect_type: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetConnectionType");
    let backend = api(connect);
    let connection = match backend.connection_info(connect) {
        Ok(connection) => connection,
        Err(error) => return error_code(error),
    };
    if connect_type.is_null() {
        return crate::last_error::null_param(2);
    }
    let server_type = match connection.server_type {
        _ if backend.is_remote() => ADS_REMOTE_SERVER,
        _ if backend::is_inside(connect) => ADS_AIS_SERVER,
        ServerType::Local => ADS_LOCAL_SERVER,
    };
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *connect_type = server_type };
    AE_SUCCESS
}

/// `AdsGetConnectionPath`: the path a connection was opened on, as given in
/// the connect string's `Data Source` (for a server, the server path).
///
/// # Safety
///
/// `path` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetConnectionPath(
    connect: AdsHandle,
    path: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetConnectionPath");
    let source = match api(connect) {
        // A hosted session (an external procedure's) has no path of its own.
        Backend::Remote(remote) if remote.connection == connect && !remote.path.is_empty() => {
            Ok(remote.path.clone())
        }
        backend => backend
            .connection_info(connect)
            .map(|connection| connection.path),
    };
    match source {
        // The path came in through the ANSI code page, so it goes back out.
        Ok(source) => {
            let source = ansi::from_os(OsStr::new(&source)).unwrap_or_default();
            // SAFETY: `path` and `len` are as # Safety says.
            unsafe { write_string_at(&source, path, len, 2) }
        }
        Err(error) => error_code(error),
    }
}

/// The server a connection's path names, as the original engine reports it
/// for its local server: the host of a UNC or server path
/// (`\\server\share` is `server`), else the drive (`C:`); a relative
/// path is on the drive of the current directory.
fn server_name(path: &str) -> String {
    fn drive(path: &str) -> Option<String> {
        let mut chars = path.chars();
        match (chars.next(), chars.next()) {
            (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => Some(format!("{letter}:")),
            _ => None,
        }
    }
    let of = |path: &str| {
        server_path(path)
            .map(|server| server.host)
            .or_else(|| drive(path))
    };
    of(path)
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|dir| of(&dir.to_string_lossy()))
        })
        .unwrap_or_default()
}

/// `AdsGetServerName`: the server a connection is on, in `name` of `*len`
/// bytes (NUL-terminated); `*len` is set to its length. For a connection to
/// `tflipd` it is the server's host as the path names it; for the local
/// server, as the original engine reports it, the host of a UNC path or else
/// the drive (`C:`).
///
/// As in the original engine: a buffer that is too small fails with
/// `AE_INSUFFICIENT_BUFFER` after the bytes that fit are copied (without a
/// NUL), and `*len` set to the length needed including the NUL; a handle
/// that is not a connection fails with `AE_INVALID_CONNECTION_HANDLE`.
///
/// # Safety
///
/// `name` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetServerName(
    connect: AdsHandle,
    name: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetServerName");
    if !is_connection(connect) {
        return fail_handle(AE_INVALID_CONNECTION_HANDLE);
    }
    let path = match api(connect) {
        Backend::Remote(remote) => Ok(remote.path.clone()),
        backend => backend
            .connection_info(connect)
            .map(|connection| connection.path),
    };
    match path {
        Ok(path) => {
            let server = ansi::from_str_lossy(&server_name(&path));
            // SAFETY: `name` and `len` are as # Safety says.
            unsafe { write_cut_at(&server, name, len, 2) }
        }
        Err(error) => error_code(error),
    }
}

/// The find handle reported when nothing matches, as the original engine
/// does (-1).
const NO_FIND: AdsHandle = AdsHandle::MAX;

/// Whether `connect` is a connection handle.
fn is_connection(connect: AdsHandle) -> bool {
    api(connect).handle_kind(connect) == Ok(Some(HandleKind::Connection))
}

/// The next name of the listing `find` that the ANSI code page can
/// represent, and whether the listing lists a data dictionary's tables;
/// names it cannot represent are skipped, as clients could not open them by
/// name.
fn next_ansi_name(find: Handle) -> Result<Option<(Vec<u8>, bool)>, Error> {
    let backend = api(find);
    while let Some((name, dictionary)) = backend.find_next_table(find)? {
        if let Some(name) = ansi::from_os(OsStr::new(&name)) {
            return Ok(Some((name, dictionary)));
        }
    }
    Ok(None)
}

/// The first name of a new listing (`name`, with the listing `find`) that
/// the ANSI code page can represent, or `None` (with the listing closed) if
/// there is none.
fn first_ansi_name(name: &str, find: Handle) -> Option<Vec<u8>> {
    let name = ansi::from_os(OsStr::new(name))
        .or_else(|| next_ansi_name(find).ok().flatten().map(|(name, _)| name));
    if name.is_none() {
        let _ = api(find).find_close(find);
    }
    name
}

/// `AdsFindFirstTable`: starts listing the tables matching `file_mask` in a
/// connection's directory, returning the first name and a handle for
/// `AdsFindNextTable`.
///
/// Names are bare file names as stored, extension included, in the file
/// system's order; every matching regular file counts. Without a match,
/// `*find` is -1 and `AE_NO_FILE_FOUND` is returned. If the first name does
/// not fit, `AE_INSUFFICIENT_BUFFER` is returned with `*file_len` set to the
/// size needed (including the NUL), the name is skipped, and the listing is
/// still opened. A null mask returns `AE_SUCCESS` without writing anything,
/// except on a connection to a data dictionary: there it lists the
/// dictionary's tables and views, with `*file_len` counting the NUL. These
/// match what the original engine does.
///
/// Unlike the original engine, which resolves a relative mask such as
/// `*.adt` or `sub\*.adt` against the process's current directory, tflip
/// resolves it against the connection's path.
///
/// # Safety
///
/// `file_mask` must be null or point to a NUL-terminated string; `first_file`
/// must be null or hold `*file_len` bytes; `file_len` must be null or valid
/// for reads and writes; `find` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsFindFirstTable(
    connect: AdsHandle,
    file_mask: *mut Unsigned8,
    first_file: *mut Unsigned8,
    file_len: *mut Unsigned16,
    find: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsFindFirstTable");
    // SAFETY: `file_mask` is null or NUL-terminated, as # Safety says.
    let mask = unsafe { text(file_mask) };
    let outs = !first_file.is_null() && !file_len.is_null() && !find.is_null();
    let first = find_first(connect, mask, outs, false);
    // SAFETY: the buffer, its length and `find` are as # Safety says.
    let (rc, dictionary, _) = unsafe { first.write(first_file, file_len, find) };
    match dictionary {
        // SAFETY: `write` only succeeds with a `file_len` that is not null.
        true => unsafe { counting_nul(rc, file_len) },
        false => rc,
    }
}

/// What [`find_first`] found.
enum FirstTable {
    /// The call's code, with nothing to write.
    Done(Unsigned32),
    /// No table matches.
    None,
    /// The first table's name, the listing, whether the listing lists a data
    /// dictionary's tables, and the name of the link the table is in, if it
    /// is a link's.
    Found {
        name: Vec<u8>,
        find: AdsHandle,
        dictionary: bool,
        link: Option<Vec<u8>>,
    },
}

impl FirstTable {
    /// Writes the first name and the listing to the caller's `first_file`,
    /// `file_len` and `find`: the code, whether the listing lists a data
    /// dictionary's tables, and the name of the link the first table is in.
    ///
    /// # Safety
    ///
    /// Unless `self` is `Done`, `first_file` must hold `*file_len` bytes,
    /// `file_len` must be valid for reads and writes and `find` for writes.
    unsafe fn write(
        self,
        first_file: *mut Unsigned8,
        file_len: *mut Unsigned16,
        find: *mut AdsHandle,
    ) -> (Unsigned32, bool, Option<Vec<u8>>) {
        match self {
            FirstTable::Done(rc) => (rc, false, None),
            FirstTable::None => {
                // SAFETY: writable, as the caller promises.
                unsafe { *find = NO_FIND };
                (fail(AE_NO_FILE_FOUND), false, None)
            }
            FirstTable::Found {
                name,
                find: handle,
                dictionary,
                link,
            } => {
                // Like the original engine: a name that does not fit the
                // buffer is lost, but the listing stays open.
                // SAFETY: as the caller promises.
                let rc = unsafe { write_name(&name, first_file, file_len) };
                // SAFETY: writable, as the caller promises.
                unsafe { *find = handle };
                (rc, dictionary, link)
            }
        }
    }
}

/// [`AdsFindFirstTable`], but for writing what it finds; `outs` says
/// whether the buffers it is written to are given. With `links`
/// (`AdsFindFirstTable62`), a dictionary's listing has its links' tables too.
fn find_first(connect: AdsHandle, mask: Option<String>, outs: bool, links: bool) -> FirstTable {
    if !is_connection(connect) {
        return FirstTable::Done(fail_handle(AE_INVALID_CONNECTION_HANDLE));
    }
    // Like the original engine: without a mask, succeed without writing
    // anything, but list a dictionary's tables.
    let dictionary = api(connect)
        .connection_info(connect)
        .is_ok_and(|c| c.dictionary);
    if mask.is_none() && !dictionary {
        return FirstTable::Done(AE_SUCCESS);
    }
    if !outs {
        return FirstTable::Done(fail(AE_INVALID_POINTER));
    }

    let mask = remote::relative_name(connect, &mask.unwrap_or_default());
    let first = match api(connect).find_first_table(connect, mask, links) {
        Ok(first) => first,
        Err(error) => return FirstTable::Done(error_code(error)),
    };
    let first = first.and_then(|(name, find, dictionary)| {
        Some((find, first_ansi_name(&name, find)?, dictionary))
    });
    match first {
        None => FirstTable::None,
        Some((find, name, dictionary)) => {
            if dictionary {
                dictionary_finds().insert(find);
            }
            let (name, link) = split_link(name);
            FirstTable::Found {
                name,
                find,
                dictionary,
                link,
            }
        }
    }
}

/// The listings of a data dictionary's tables still open, whose end the
/// original engine reports as `AE_NO_FILE_FOUND` with `AE_NO_OBJECT_FOUND`
/// as the last error.
fn dictionary_finds() -> MutexGuard<'static, std::collections::BTreeSet<AdsHandle>> {
    static FINDS: Mutex<std::collections::BTreeSet<AdsHandle>> =
        Mutex::new(std::collections::BTreeSet::new());
    FINDS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The end of the listing `find`: `AE_NO_FILE_FOUND`, the last error saying
/// so as the original engine does for a dictionary's listing.
fn listing_end(find: AdsHandle) -> Unsigned32 {
    match dictionary_finds().contains(&find) {
        true => {
            fail(crate::AE_NO_OBJECT_FOUND);
            AE_NO_FILE_FOUND
        }
        false => fail(AE_NO_FILE_FOUND),
    }
}

/// A listed table's name and the link it is in: `:link::table` is a link's
/// table (see `tflip_engine::link`).
fn split_link(name: Vec<u8>) -> (Vec<u8>, Option<Vec<u8>>) {
    let Some(rest) = name.strip_prefix(b":") else {
        return (name, None);
    };
    match rest.windows(2).position(|pair| pair == b"::") {
        Some(at) => (rest[at + 2..].to_vec(), Some(rest[..at].to_vec())),
        None => (name, None),
    }
}

/// `AdsFindNextTable`: the next name of a listing started by
/// `AdsFindFirstTable`, or `AE_NO_FILE_FOUND` after the last one. A name that
/// does not fit returns `AE_INSUFFICIENT_BUFFER` with `*file_len` set to the
/// size needed (including the NUL) and is skipped, as in the original engine.
///
/// An unknown find handle returns `AE_INVALID_HANDLE`; the original engine
/// returns `AE_SUCCESS` there without a name.
///
/// # Safety
///
/// `file_name` must be null or hold `*file_len` bytes; `file_len` must be
/// null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsFindNextTable(
    connect: AdsHandle,
    find: AdsHandle,
    file_name: *mut Unsigned8,
    file_len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsFindNextTable");
    if !is_connection(connect) {
        return fail_handle(AE_INVALID_CONNECTION_HANDLE);
    }
    match next_ansi_name(find) {
        Ok(Some((name, dictionary))) => {
            // SAFETY: `file_name` and `file_len` are as # Safety says.
            let rc = unsafe { write_name(&name, file_name, file_len) };
            match dictionary {
                // SAFETY: `write_name` only succeeds with a `file_len` that
                // is not null.
                true => unsafe { counting_nul(rc, file_len) },
                false => rc,
            }
        }
        Ok(None) => listing_end(find),
        Err(error) => error_code(error),
    }
}

/// `rc` with the length a name was written with counting its NUL, as a
/// listing of a dictionary's tables reports it (not `AdsFindFirstTable62`'s).
///
/// # Safety
///
/// `len` must be valid for reads and writes if `rc` is `AE_SUCCESS`.
unsafe fn counting_nul(rc: Unsigned32, len: *mut Unsigned16) -> Unsigned32 {
    if rc == AE_SUCCESS {
        // SAFETY: as the caller promises.
        unsafe { *len += 1 };
    }
    rc
}

/// Writes an empty name of the link a table listing found a table in
/// (`AdsFindFirstTable62`, `AdsFindNextTable62`), as the original engine does
/// for a table of the connection's own (and on a free connection).
///
/// # Safety
///
/// `dd_name` must be null or hold `*dd_len` bytes; `dd_len` must be null or
/// valid for reads and writes.
unsafe fn write_no_dictionary(dd_name: *mut Unsigned8, dd_len: *mut Unsigned16) -> Unsigned32 {
    if dd_name.is_null() || dd_len.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: neither is null, so `dd_len` is readable and writable and
    // `dd_name` holds `*dd_len` bytes.
    unsafe {
        if *dd_len > 0 {
            *dd_name = 0;
        }
        *dd_len = 0;
    }
    AE_SUCCESS
}

/// Writes `link`, the name of the link a listed table is in, if it is a
/// link's, else nothing (see [`write_no_dictionary`]).
///
/// # Safety
///
/// `dd_name` must hold `dd_size` bytes, and `dd_len` must be valid for reads
/// and writes.
unsafe fn write_link(
    link: Option<&[u8]>,
    dd_name: *mut Unsigned8,
    dd_len: *mut Unsigned16,
    dd_size: Unsigned16,
) -> Unsigned32 {
    match link {
        // SAFETY: as the caller promises.
        Some(link) => unsafe {
            *dd_len = dd_size;
            write_name(link, dd_name, dd_len)
        },
        None => AE_SUCCESS,
    }
}

/// `AdsFindFirstTable62`: [`AdsFindFirstTable`], also returning the name of
/// the link the table is in, in `first_dd` of `*dd_len` bytes: on a
/// dictionary connection without a mask, the listing has the tables of the
/// dictionary's links too (see `tflip_engine::link`); for its own tables the
/// name is empty.
///
/// # Safety
///
/// As for [`AdsFindFirstTable`]; `first_dd` must be null or hold `*dd_len`
/// bytes; `dd_len` must be null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsFindFirstTable62(
    connect: AdsHandle,
    file_mask: *mut Unsigned8,
    first_dd: *mut Unsigned8,
    dd_len: *mut Unsigned16,
    first_file: *mut Unsigned8,
    file_len: *mut Unsigned16,
    find: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsFindFirstTable62");
    if !is_connection(connect) {
        return fail_handle(AE_INVALID_CONNECTION_HANDLE);
    }
    // Unlike `AdsFindFirstTable`, without counting the NUL.
    // SAFETY: `dd_len` is null or readable, as # Safety says.
    let dd_size = unsafe { dd_len.as_ref() }.copied().unwrap_or(0);
    // SAFETY: `first_dd` and `dd_len` are as # Safety says.
    match unsafe { write_no_dictionary(first_dd, dd_len) } {
        AE_SUCCESS => {
            // SAFETY: `file_mask` is null or NUL-terminated, as # Safety
            // says.
            let mask = unsafe { text(file_mask) };
            let outs = !first_file.is_null() && !file_len.is_null() && !find.is_null();
            let first = find_first(connect, mask, outs, true);
            // SAFETY: the buffer, its length and `find` are as # Safety
            // says.
            let (rc, _, link) = unsafe { first.write(first_file, file_len, find) };
            match rc {
                // SAFETY: `write_no_dictionary` succeeded, so neither is
                // null, and `first_dd` holds `dd_size` bytes.
                AE_SUCCESS => unsafe { write_link(link.as_deref(), first_dd, dd_len, dd_size) },
                rc => rc,
            }
        }
        rc => rc,
    }
}

/// `AdsFindNextTable62`: [`AdsFindNextTable`], also returning the name of
/// the link the table is in, in `dd_name` of `*dd_len` bytes (see
/// [`AdsFindFirstTable62`]). Unlike
/// `AdsFindNextTable`, an unknown find handle fails with `AE_NO_FILE_FOUND`,
/// as in the original engine.
///
/// # Safety
///
/// As for [`AdsFindNextTable`]; `dd_name` must be null or hold `*dd_len`
/// bytes; `dd_len` must be null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsFindNextTable62(
    connect: AdsHandle,
    find: AdsHandle,
    dd_name: *mut Unsigned8,
    dd_len: *mut Unsigned16,
    file_name: *mut Unsigned8,
    file_len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsFindNextTable62");
    if !is_connection(connect) {
        return fail_handle(AE_INVALID_CONNECTION_HANDLE);
    }
    if dd_name.is_null() || dd_len.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // The dictionary's name is written only with a table's.
    match next_ansi_name(find) {
        Ok(Some((name, _))) => {
            // SAFETY: not null, so readable, as # Safety says.
            let dd_size = unsafe { *dd_len };
            // SAFETY: `dd_name` and `dd_len` are as # Safety says.
            let rc = unsafe { write_no_dictionary(dd_name, dd_len) };
            if rc != AE_SUCCESS {
                return rc;
            }
            let (name, link) = split_link(name);
            // SAFETY: `file_name` and `file_len` are as # Safety says.
            match unsafe { write_name(&name, file_name, file_len) } {
                // SAFETY: neither is null, and `dd_name` holds `dd_size`
                // bytes, as # Safety says.
                AE_SUCCESS => unsafe { write_link(link.as_deref(), dd_name, dd_len, dd_size) },
                rc => rc,
            }
        }
        Ok(None) => listing_end(find),
        Err(Error::InvalidHandle) => fail(AE_NO_FILE_FOUND),
        Err(error) => error_code(error),
    }
}

/// `AdsFindClose`: ends a listing started by `AdsFindFirstTable`. Like the
/// original engine, it succeeds for an unknown or already closed handle.
#[unsafe(no_mangle)]
pub extern "system" fn AdsFindClose(connect: AdsHandle, find: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsFindClose");
    if !is_connection(connect) {
        return fail_handle(AE_INVALID_CONNECTION_HANDLE);
    }
    dictionary_finds().remove(&find);
    let _ = api(connect).find_close(find);
    AE_SUCCESS
}

/// `AdsDisconnect`: closes a connection and everything opened on it; 0
/// closes every connection, those to servers included.
///
/// A handle that is not an open connection returns
/// `AE_INVALID_CONNECTION_HANDLE`, while `AdsGetLastError` reports
/// `AE_INVALID_HANDLE`, as the original engine does for a closed connection.
/// (It crashes on a value that never was a handle.)
#[unsafe(no_mangle)]
pub extern "system" fn AdsDisconnect(connect: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDisconnect");
    let result = match connect {
        0 => {
            backend::disconnect_all_remotes();
            LOCAL.disconnect(0)
        }
        _ if api(connect).is_remote() => backend::disconnect_remote(connect),
        _ => LOCAL.disconnect(connect),
    };
    if result.is_ok() {
        backend::forget_inside(connect);
        crate::connect_options::forget(connect);
        match connect {
            0 => passwords().clear(),
            _ => drop(passwords().remove(&connect)),
        }
    }
    match result {
        Ok(()) => AE_SUCCESS,
        Err(tflip_engine::Error::InvalidConnection) => {
            fail_handle(AE_INVALID_HANDLE);
            AE_INVALID_CONNECTION_HANDLE
        }
        Err(error) => error_code(error),
    }
}

/// The engine of the connection `connect`, checked as the original engine
/// checks a connection handle: any other handle fails with
/// `AE_INVALID_CONNECTION_HANDLE`, with `AE_INVALID_HANDLE` as the last
/// error for a value that is no handle. The management functions take a
/// connection in place of a management handle, as the original engine
/// does.
pub(crate) fn checked(connect: AdsHandle) -> Result<Backend, Unsigned32> {
    let backend = api(connect);
    match backend.handle_kind(connect) {
        Ok(Some(HandleKind::Connection)) => Ok(backend),
        Ok(None) => {
            fail_handle(AE_INVALID_HANDLE);
            Err(AE_INVALID_CONNECTION_HANDLE)
        }
        Ok(Some(_)) => Err(fail_handle(AE_INVALID_CONNECTION_HANDLE)),
        Err(error) => Err(error_code(error)),
    }
}

/// `AdsCloseCachedTables`: closes the tables a connection keeps open for
/// reuse after they were closed (`AdsCacheOpenTables`); not those of its
/// closed SQL statements (`AdsCacheOpenCursors`), and tables that are open
/// stay open, as in the original engine. The local server keeps none, so
/// there is nothing to close there. A handle that is not a connection fails
/// as `checked` says.
#[unsafe(no_mangle)]
pub extern "system" fn AdsCloseCachedTables(connect: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCloseCachedTables");
    match checked(connect) {
        Ok(backend) => code(backend.close_cached_tables(connect)),
        Err(code) => code,
    }
}

/// `AdsGetConnectionProperty`: a property of a connection, in `value` of
/// `*len` bytes; `*len` is set to its length. As the original engine
/// reports them:
///
/// - `ADS_CONNECTIONPROP_USERNAME` and `ADS_CONNECTIONPROP_PASSWORD`: what
///   the connection was opened with (`AdsConnect60`, or `User ID` and
///   `Password` in a connect string), as text; empty if nothing was given.
/// - `ADS_CONNECTIONPROP_PROTOCOL`: a 4-byte number, 0 for the local server;
///   `ADS_CONNECTIONPROP_USING_TLC` (5) for `tflipd`, which always uses TLS.
/// - `ADS_CONNECTIONPROP_ENCRYPTION_TYPE`: `RC4`, the original engine's
///   default.
/// - `ADS_CONNECTIONPROP_FIPS_MODE`: a 2-byte number, 0.
/// - `ADS_CONNECTIONPROP_CIPHER_SUITE`: the TLS cipher suite for `tflipd`
///   (`TLS13_AES_256_GCM_SHA384`), else empty; the certificate file and
///   common name are empty.
///
/// Text is NUL-terminated, and `*len` does not count the NUL; text that does
/// not fit fails with `AE_INSUFFICIENT_BUFFER` after the bytes that fit are
/// copied (without a NUL), and `*len` set to the length needed with the NUL.
/// A number that does not fit fails with `AE_INVALID_PROPERTY_LENGTH`,
/// another property with `AE_INVALID_OPTION`, and a handle that is not a
/// connection with `AE_INVALID_CONNECTION_HANDLE`.
///
/// # Safety
///
/// `value` must be null or hold `*len` bytes; `len` must be null or valid
/// for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetConnectionProperty(
    connect: AdsHandle,
    property: Unsigned16,
    value: *mut c_void,
    len: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetConnectionProperty");
    let backend = match checked(connect) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    if value.is_null() || len.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    match connection_property(connect, &backend, property) {
        // SAFETY: neither is null, so as # Safety says.
        Ok(Property::Number(bytes)) => unsafe { write_number(&bytes, value.cast(), len) },
        Ok(Property::Text(text)) => {
            let bytes = ansi::from_str_lossy(&text);
            // SAFETY: neither is null, so as # Safety says.
            unsafe { crate::ffi::write_units(&bytes, value.cast(), len) }
        }
        Err(code) => code,
    }
}

/// A connection property's value (see [`AdsGetConnectionProperty`]).
enum Property {
    /// A number, as its bytes.
    Number(Vec<u8>),
    Text(String),
}

/// The property `property` of the connection `connect` on `backend`.
fn connection_property(
    connect: AdsHandle,
    backend: &Backend,
    property: Unsigned16,
) -> Result<Property, Unsigned32> {
    let password = passwords().get(&connect).cloned();
    let user = backend
        .connection_info(connect)
        .ok()
        .and_then(|info| info.user);
    let remote = match backend {
        Backend::Remote(remote) => Some(Arc::clone(remote)),
        Backend::Local | Backend::Foreign => None,
    };
    let text = |text: Option<&str>| Ok(Property::Text(text.unwrap_or_default().to_owned()));
    match property {
        ADS_CONNECTIONPROP_USERNAME => text(user.as_deref()),
        ADS_CONNECTIONPROP_PASSWORD => text(password.as_deref()),
        ADS_CONNECTIONPROP_PROTOCOL => {
            let protocol: Unsigned32 = if remote.is_some() {
                ADS_CONNECTIONPROP_USING_TLC
            } else {
                0
            };
            Ok(Property::Number(protocol.to_le_bytes().to_vec()))
        }
        ADS_CONNECTIONPROP_ENCRYPTION_TYPE => text(Some("RC4")),
        ADS_CONNECTIONPROP_FIPS_MODE => Ok(Property::Number(0u16.to_le_bytes().to_vec())),
        ADS_CONNECTIONPROP_CERTIFICATE_FILE | ADS_CONNECTIONPROP_COMMON_NAME => text(None),
        ADS_CONNECTIONPROP_CIPHER_SUITE => text(
            remote
                .as_ref()
                .and_then(|remote| remote.client.cipher_suite()),
        ),
        _ => Err(fail(AE_INVALID_OPTION)),
    }
}

/// Writes a number property's `bytes` to `value` of `*len` bytes, setting
/// `*len` to their length; a buffer they do not fit fails with
/// `AE_INVALID_PROPERTY_LENGTH`.
///
/// # Safety
///
/// `value` must hold `*len` bytes; `len` must be valid for reads and writes.
unsafe fn write_number(bytes: &[u8], value: *mut Unsigned8, len: *mut Unsigned32) -> Unsigned32 {
    // SAFETY: as the caller promises; the copy stays inside `*len` bytes.
    unsafe {
        if (*len as usize) < bytes.len() {
            return fail(AE_INVALID_PROPERTY_LENGTH);
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), value, bytes.len());
        *len = bytes.len() as Unsigned32;
    }
    AE_SUCCESS
}

pub const ADS_CONNECTIONPROP_USERNAME: Unsigned16 = 0;
pub const ADS_CONNECTIONPROP_PASSWORD: Unsigned16 = 1;
pub const ADS_CONNECTIONPROP_PROTOCOL: Unsigned16 = 2;
pub const ADS_CONNECTIONPROP_ENCRYPTION_TYPE: Unsigned16 = 3;
pub const ADS_CONNECTIONPROP_FIPS_MODE: Unsigned16 = 4;
pub const ADS_CONNECTIONPROP_CERTIFICATE_FILE: Unsigned16 = 5;
pub const ADS_CONNECTIONPROP_CIPHER_SUITE: Unsigned16 = 6;
pub const ADS_CONNECTIONPROP_COMMON_NAME: Unsigned16 = 7;
pub const ADS_CONNECTIONPROP_USING_TCP_IP: Unsigned32 = 1;
pub const ADS_CONNECTIONPROP_USING_TLC: Unsigned32 = 5;

/// Checks that `connect` is a connection with a data dictionary
/// (`AE_NOT_DICTIONARY` for a free-table connection).
fn dictionary_connection(connect: AdsHandle) -> Result<(), Unsigned32> {
    let backend = checked(connect)?;
    match backend.connection_info(connect) {
        Ok(info) if info.dictionary => Ok(()),
        Ok(_) => Err(fail(crate::AE_NOT_DICTIONARY)),
        Err(error) => Err(error_code(error)),
    }
}

/// `AdsGetNumActiveLinks`: the links to other dictionaries the connection
/// has active (see `tflip_engine::link`); a connection without a data
/// dictionary fails with `AE_NOT_DICTIONARY`, as in the original engine.
///
/// # Safety
///
/// `links` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetNumActiveLinks(
    connect: AdsHandle,
    links: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetNumActiveLinks");
    if let Err(code) = dictionary_connection(connect) {
        return code;
    }
    if links.is_null() {
        return crate::last_error::null_param(2);
    }
    match api(connect).active_links(connect) {
        Ok(active) => {
            let count = Unsigned16::try_from(active.len()).unwrap_or(Unsigned16::MAX);
            // SAFETY: not null, so writable, as # Safety says.
            unsafe { *links = count };
            AE_SUCCESS
        }
        Err(error) => error_code(error),
    }
}

/// `AdsGetActiveLinkInfo`: the active link numbered `link` (from 1, the
/// newest) of a dictionary connection, as the original engine writes it:
/// `name;user;dictionary` and a NUL, `*len` set to its length with the
/// NUL. A buffer it does not fit gets what fits, without the NUL, and
/// `AE_INSUFFICIENT_BUFFER`; any other number fails with
/// `AE_INVALID_LINK_NUMBER`. As there: a connection without a data dictionary fails with
/// `AE_NOT_DICTIONARY` (so does any other handle, recording
/// `AE_INVALID_HANDLE`), and a null `len` (checked before `info`) with
/// `AE_INVALID_POINTER`.
///
/// # Safety
///
/// `info` and `len` must be null or valid.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetActiveLinkInfo(
    connect: AdsHandle,
    link: Unsigned16,
    info: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetActiveLinkInfo");
    // What is no connection fails as one without a dictionary, with the
    // handle's error as the last one.
    if checked(connect).is_err() {
        crate::last_error::fail_text(
            AE_INVALID_HANDLE,
            "Error 5018:  The handle given was not recognized by Advantage. Verify specified \
             handle is open/active.  The given handle is not readable."
                .to_owned(),
        );
        return crate::AE_NOT_DICTIONARY;
    }
    if let Err(code) = dictionary_connection(connect) {
        return code;
    }
    for (pointer, number) in [(len.is_null(), 4), (info.is_null(), 3)] {
        if pointer {
            let text = format!("The pointer given was not valid.  Parameter {number}");
            return crate::last_error::fail_text(
                AE_INVALID_POINTER,
                format!("Error {AE_INVALID_POINTER}:  {text}"),
            );
        }
    }
    let active = match api(connect).active_links(connect) {
        Ok(active) => active,
        Err(error) => return error_code(error),
    };
    let Some((name, user, path)) = usize::from(link).checked_sub(1).and_then(|i| active.get(i))
    else {
        return fail(crate::AE_INVALID_LINK_NUMBER);
    };
    let text = ansi::from_str_lossy(&format!("{name};{user};{path}"));
    // SAFETY: neither is null, so `info` holds `*len` bytes and `len` is
    // readable and writable, as # Safety says.
    unsafe { write_counting_nul(&text, info, len) }
}

/// Writes `text` and a NUL to `buf` of `*len` bytes, setting `*len` to its
/// length with the NUL; a buffer it does not fit gets what fits, without
/// the NUL, and `AE_INSUFFICIENT_BUFFER`.
///
/// # Safety
///
/// `buf` must hold `*len` bytes; `len` must be valid for reads and writes.
unsafe fn write_counting_nul(text: &[u8], buf: *mut Unsigned8, len: *mut Unsigned16) -> Unsigned32 {
    let needed = Unsigned16::try_from(text.len() + 1).unwrap_or(Unsigned16::MAX);
    // SAFETY: as the caller promises; the copies stay inside `*len` bytes.
    unsafe {
        let size = usize::from(*len);
        if text.len() >= size {
            std::ptr::copy_nonoverlapping(text.as_ptr(), buf, size.min(text.len()));
            *len = needed;
            return fail(crate::AE_INSUFFICIENT_BUFFER);
        }
        std::ptr::copy_nonoverlapping(text.as_ptr(), buf, text.len());
        *buf.add(text.len()) = 0;
        *len = needed;
    }
    AE_SUCCESS
}

/// `AdsFindServers`: a table of the Advantage servers on the network, found by
/// IPv4 multicast (`224.0.1.55:2989`). tflip fills the address, port, multicast
/// address and server-name columns from each reply and leaves the ten detail
/// columns blank — the original gathers those by then connecting to each
/// server, which tflip has no remote client to do, so it behaves like the
/// original's `ADS_FS_MULTICAST_ONLY` (see [`crate::find_servers`]).
///
/// Options other than `ADS_FS_MULTICAST_ONLY` and `ADS_FS_CONNECT_ALL` fail
/// with `AE_INVALID_OPTION`, a null `table` with `AE_INVALID_POINTER`. With no
/// servers found tflip returns an empty table and `AE_SUCCESS`, the documented
/// `phTable` contract; the original instead returns the system error `50` with
/// a NULL handle (a deliberate difference — see `docs/ADS_BUGS.md` and
/// `docs/DIFFERENCES.md`).
///
/// # Safety
///
/// `table` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsFindServers(
    options: Unsigned32,
    table: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsFindServers");
    let invalid = |code: Unsigned32, text: &str, number: u8| {
        crate::last_error::fail_text(code, format!("Error {code}:  {text}  Parameter {number}"))
    };
    if options & !0x3 != 0 {
        return invalid(AE_INVALID_OPTION, "An invalid option was specified.", 1);
    }
    // SAFETY: `table` is null or writable, as # Safety says.
    let Some(table) = (unsafe { table.as_mut() }) else {
        return invalid(AE_INVALID_POINTER, "The pointer given was not valid.", 2);
    };
    match crate::find_servers::find_servers() {
        Ok(handle) => {
            *table = handle;
            AE_SUCCESS
        }
        Err(error) => {
            *table = 0;
            error_code(error)
        }
    }
}

/// `AdsDisableRI`: turns off referential integrity on a dictionary
/// connection; tflip enforces none, so nothing changes. A connection
/// without a data dictionary fails with `AE_NOT_DICTIONARY`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsDisableRI(connect: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDisableRI");
    dictionary_connection(connect).err().unwrap_or(AE_SUCCESS)
}

/// `AdsEnableRI`: as [`AdsDisableRI`].
#[unsafe(no_mangle)]
pub extern "system" fn AdsEnableRI(connect: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsEnableRI");
    dictionary_connection(connect).err().unwrap_or(AE_SUCCESS)
}

/// A switch on a connection that tflip takes but does not act on.
fn accepted(connect: AdsHandle) -> Unsigned32 {
    checked(connect).err().unwrap_or(AE_SUCCESS)
}

/// `AdsDisableUniqueEnforcement`: lets the connection's writes add a key an
/// ADI-unique or CDX-candidate order already has, instead of refusing it.
#[unsafe(no_mangle)]
pub extern "system" fn AdsDisableUniqueEnforcement(connect: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDisableUniqueEnforcement");
    match checked(connect) {
        Ok(backend) => code(backend.set_unique_enforcement(connect, false)),
        Err(code) => code,
    }
}

/// `AdsEnableUniqueEnforcement`: restores unique-key enforcement on the
/// connection's writes (the default).
#[unsafe(no_mangle)]
pub extern "system" fn AdsEnableUniqueEnforcement(connect: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsEnableUniqueEnforcement");
    match checked(connect) {
        Ok(backend) => code(backend.set_unique_enforcement(connect, true)),
        Err(code) => code,
    }
}

/// `AdsDisableAutoIncEnforcement`: lets the connection's writes set an AutoInc
/// field to their own value instead of the engine filling it.
#[unsafe(no_mangle)]
pub extern "system" fn AdsDisableAutoIncEnforcement(connect: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDisableAutoIncEnforcement");
    match checked(connect) {
        Ok(backend) => code(backend.set_autoinc_enforcement(connect, false)),
        Err(code) => code,
    }
}

/// `AdsEnableAutoIncEnforcement`: restores the engine filling AutoInc fields on
/// the connection's writes (the default).
#[unsafe(no_mangle)]
pub extern "system" fn AdsEnableAutoIncEnforcement(connect: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsEnableAutoIncEnforcement");
    match checked(connect) {
        Ok(backend) => code(backend.set_autoinc_enforcement(connect, true)),
        Err(code) => code,
    }
}

/// `AdsSetFlushFlag` (unpublished): whether every update is flushed to disk;
/// tflip writes through to the files anyway.
#[unsafe(no_mangle)]
pub extern "system" fn AdsSetFlushFlag(connect: AdsHandle, _flush: Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetFlushFlag");
    accepted(connect)
}

/// Finds the first open connection whose path `matches` accepts (local
/// connections first), for `AdsFindConnection` and `AdsFindConnection25`:
/// none fails with `AE_NO_CONNECTION` and sets `*connect` to 0, as in the
/// original engine; a null `name` (`None`) fails with `AE_INVALID_POINTER`.
fn find_connection(
    name: Option<String>,
    connect: Option<&mut AdsHandle>,
    matches: impl Fn(&str, &str) -> bool,
) -> Unsigned32 {
    let Some(name) = name else {
        let text = "The pointer given was not valid.  Parameter 1";
        return crate::last_error::fail_text(
            AE_INVALID_POINTER,
            format!("Error {AE_INVALID_POINTER}:  {text}"),
        );
    };
    let Some(connect) = connect else {
        return fail(AE_INVALID_POINTER);
    };
    let name = remote::universal_name(&name);
    let local = tflip_engine::handle::connections(&LOCAL);
    let remotes = backend::all_remotes();
    let remotes = remotes
        .iter()
        .map(|remote| (remote.connection, remote.path.clone()));
    let found = local
        .into_iter()
        .chain(remotes)
        .find(|(_, path)| matches(&name, path));
    match found {
        Some((handle, _)) => {
            *connect = handle;
            AE_SUCCESS
        }
        None => {
            *connect = 0;
            fail(crate::AE_NO_CONNECTION)
        }
    }
}

/// `AdsFindConnection`: the first open connection to the server `server`
/// names, as [`AdsGetServerName`] names servers (any path on the server
/// will do: for the local server, any path on the drive), regardless of
/// case.
///
/// # Safety
///
/// `server` must be null or NUL-terminated; `connect` null or valid for
/// writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsFindConnection(
    server: *mut Unsigned8,
    connect: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsFindConnection");
    let same = |name: &str, path: &str| server_name(name).eq_ignore_ascii_case(&server_name(path));
    // SAFETY: `server` is null or NUL-terminated and `connect` null or
    // writable, as # Safety says.
    let (server, connect) = unsafe { (text(server), connect.as_mut()) };
    find_connection(server, connect, same)
}

/// `AdsFindConnection25`: the first open connection to the folder or data
/// dictionary `path` names, regardless of case and of a trailing separator
/// (a table in the folder is not the folder, as in the original engine).
///
/// # Safety
///
/// As [`AdsFindConnection`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsFindConnection25(
    path: *mut Unsigned8,
    connect: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsFindConnection25");
    let normal = |path: &str| path.replace('\\', "/").trim_end_matches('/').to_lowercase();
    let same = |name: &str, path: &str| normal(name) == normal(path);
    // SAFETY: `path` is null or NUL-terminated and `connect` null or
    // writable, as # Safety says.
    let (path, connect) = unsafe { (text(path), connect.as_mut()) };
    find_connection(path, connect, same)
}

/// `AdsReapUnusedConnections`: closes the connections to servers that the
/// original engine keeps open after their last use; tflip keeps none.
#[unsafe(no_mangle)]
pub extern "system" fn AdsReapUnusedConnections() -> Unsigned32 {
    let _call = Call::begin("AdsReapUnusedConnections");
    AE_SUCCESS
}

/// `AdsDisableLocalConnections`: keeps the process from connecting through
/// the local server; taken, as tflip connects as asked.
#[unsafe(no_mangle)]
pub extern "system" fn AdsDisableLocalConnections() -> Unsigned32 {
    let _call = Call::begin("AdsDisableLocalConnections");
    AE_SUCCESS
}

/// `AdsClearCachePool`: there is no pool of cached connections to clear.
///
/// # Safety
///
/// `connect_string` is not read.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsClearCachePool(connect_string: *mut Unsigned8) -> Unsigned32 {
    let _call = Call::begin("AdsClearCachePool");
    let _ = connect_string;
    AE_SUCCESS
}

/// `AdsIsConnectionAlive`: `ADS_TRUE` for an open connection; any other
/// handle fails with `AE_INVALID_CONNECTION_HANDLE`.
///
/// # Safety
///
/// `alive` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsConnectionAlive(
    connect: AdsHandle,
    alive: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsIsConnectionAlive");
    // SAFETY: `alive` is null or writable, as # Safety says.
    unsafe { answer(connection_check(connect), alive, ADS_TRUE) }
}

/// `AdsIsServerLoaded`: `ADS_TRUE`: the local server is part of the DLL
/// (servers to reach over the network are not looked for).
///
/// # Safety
///
/// `server` is not read; `loaded` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsIsServerLoaded(
    server: *mut Unsigned8,
    loaded: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsIsServerLoaded");
    let _ = server;
    // SAFETY: `loaded` is null or writable, as # Safety says.
    unsafe { answer(Ok(()), loaded, ADS_TRUE) }
}

/// `AdsResetConnection`: closes the tables the process opened on a
/// connection, as the original engine does.
#[unsafe(no_mangle)]
pub extern "system" fn AdsResetConnection(connect: AdsHandle) -> Unsigned32 {
    let _call = Call::begin("AdsResetConnection");
    if let Err(code) = connection_check(connect) {
        return code;
    }
    for handle in api(connect).tables(connect).unwrap_or_default() {
        let _ = api(handle).close_table(handle);
    }
    AE_SUCCESS
}

/// `AdsGetServerTime`: the date (in the date format), the milliseconds
/// since midnight and the time (12-hour, `01:14:15 AM`) where the
/// connection's server runs, as the original engine writes them. The time
/// is the client's.
///
/// # Safety
///
/// `date` and `time` must be null or hold `*date_len` and `*time_len`
/// bytes; the lengths and `ms` must be null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetServerTime(
    connect: AdsHandle,
    date: *mut Unsigned8,
    date_len: *mut Unsigned16,
    ms: *mut Signed32,
    time: *mut Unsigned8,
    time_len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetServerTime");
    if let Err(code) = connection_check(connect) {
        return code;
    }
    if ms.is_null() {
        return crate::last_error::null_param(4);
    }
    let (julian, milliseconds) = tflip_engine::clock::now().local;
    let text = |value: Value, field_type| {
        let layout = Layout {
            field_type,
            length: 0,
            decimals: 0,
        };
        let format = api(connect).date_format(connect).unwrap_or_default();
        match format::field_text(&value, &layout, &format) {
            FieldText::Text(text) => text,
            FieldText::Bytes(_) => String::new(),
        }
    };
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *ms = milliseconds as Signed32 };
    let date_text = text(Value::Date(julian), FieldType::Date);
    // SAFETY: `date` and `date_len` are as # Safety says.
    let rc = unsafe { write_cut_at(date_text.as_bytes(), date, date_len, 2) };
    if rc != AE_SUCCESS {
        return rc;
    }
    let time_text = text(Value::Time(milliseconds), FieldType::Time);
    // SAFETY: `time` and `time_len` are as # Safety says.
    unsafe { write_cut_at(time_text.as_bytes(), time, time_len, 5) }
}

/// `AdsThreadExit`: nothing is kept per thread to free.
#[unsafe(no_mangle)]
pub extern "system" fn AdsThreadExit() -> Unsigned32 {
    let _call = Call::begin("AdsThreadExit");
    AE_SUCCESS
}

/// `AdsApplicationExit`: closes what the process has open, local and on
/// servers, as before the DLL unloads; tables write their pending changes
/// as they close. As in the original engine (probed), every handle is
/// invalid afterwards, and it succeeds with nothing open or when called
/// again. Nothing runs when the DLL unloads, so a client that neither calls
/// this nor disconnects loses the changes it has not written.
#[unsafe(no_mangle)]
pub extern "system" fn AdsApplicationExit() -> Unsigned32 {
    let _call = Call::begin("AdsApplicationExit");
    backend::disconnect_all_remotes();
    connection::close_all(&LOCAL);
    passwords().clear();
    AE_SUCCESS
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::{Mutex, PoisonError};

    use super::*;
    use crate::{AE_INSUFFICIENT_BUFFER, REMOTE_LAYER_UNAVAILABLE};

    /// Serializes the tests that connect, as they share the engine's
    /// connection state.
    static CONNECT: Mutex<()> = Mutex::new(());

    fn connect_to(source: &str) -> AdsHandle {
        let mut s = format!("Data Source={source};ServerType=LOCAL;\0").into_bytes();
        let mut connect = 0;
        let rc = unsafe { AdsConnect101(s.as_mut_ptr(), std::ptr::null_mut(), &mut connect) };
        assert_eq!(rc, AE_SUCCESS);
        connect
    }

    /// As the original engine finds connections (probed): by the folder,
    /// regardless of case and of a trailing separator, but not by a table in
    /// it; by any path on the server; none is `AE_NO_CONNECTION` and 0.
    #[test]
    fn finds_connections() {
        use crate::fixtures;
        let source = fixtures::scratch("finds_connections", "all_types");
        let connect = fixtures::connect_to(&source);
        let find = |f: unsafe extern "system" fn(*mut Unsigned8, *mut AdsHandle) -> Unsigned32,
                    path: &str| {
            let mut path = format!("{path}\0").into_bytes();
            let mut found = 77;
            let rc = unsafe { f(path.as_mut_ptr(), &mut found) };
            (rc, found)
        };
        assert_eq!(find(AdsFindConnection25, &source), (AE_SUCCESS, connect));
        let upper = format!("{}/", source.to_uppercase());
        if !fixtures::is_remote() {
            assert_eq!(find(AdsFindConnection25, &upper).0, AE_SUCCESS);
        }
        let table = format!("{source}/all_types.adt");
        assert_eq!(
            find(AdsFindConnection25, &table),
            (crate::AE_NO_CONNECTION, 0)
        );
        let (rc, found) = find(AdsFindConnection, &table);
        assert_eq!(rc, AE_SUCCESS);
        assert_ne!(found, 0);
        let mut found = 77;
        let rc = unsafe { AdsFindConnection(std::ptr::null_mut(), &mut found) };
        assert_eq!((rc, found), (AE_INVALID_POINTER, 77));
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
        assert_eq!(
            find(AdsFindConnection25, &source),
            (crate::AE_NO_CONNECTION, 0)
        );
    }

    #[test]
    fn connection_path() {
        let _guard = CONNECT.lock().unwrap_or_else(PoisonError::into_inner);
        let connect = connect_to(r"C:\orders");

        let mut buf = [0xffu8; 16];
        let mut len = buf.len() as u16;
        assert_eq!(
            unsafe { AdsGetConnectionPath(connect, buf.as_mut_ptr(), &mut len) },
            AE_SUCCESS
        );
        assert_eq!(&buf[..10], b"C:\\orders\0");
        assert_eq!(len, 9);

        let mut small = [0u8; 9];
        let mut len = small.len() as u16;
        let rc = unsafe { AdsGetConnectionPath(connect, small.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_INSUFFICIENT_BUFFER);
        assert_eq!(len, 9);

        let rc = unsafe { AdsGetConnectionPath(0, buf.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_INVALID_CONNECTION_HANDLE);
    }

    #[test]
    fn connection_type_and_disconnect() {
        let connect = connect_to("x");
        let mut server_type = 0;
        assert_eq!(
            unsafe { AdsGetConnectionType(connect, &mut server_type) },
            AE_SUCCESS
        );
        assert_eq!(server_type, ADS_LOCAL_SERVER);

        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
        let rc = unsafe { AdsGetConnectionType(connect, &mut server_type) };
        assert_eq!(rc, AE_INVALID_CONNECTION_HANDLE);
        // A successful call first: a failed handle check keeps a pending
        // error.
        assert_eq!(crate::AdsCacheOpenCursors(25), AE_SUCCESS);
        assert_eq!(AdsDisconnect(connect), AE_INVALID_CONNECTION_HANDLE);
        let mut code = 0;
        let (buf, len) = (std::ptr::null_mut(), std::ptr::null_mut());
        assert_eq!(
            unsafe { crate::AdsGetLastError(&mut code, buf, len) },
            AE_SUCCESS
        );
        assert_eq!(
            code, AE_INVALID_HANDLE,
            "the last error differs from the result"
        );
        assert_eq!(AdsDisconnect(0x12345), AE_INVALID_CONNECTION_HANDLE);

        let mut s = *b"Data Source=x;ServerType=REMOTE;\0";
        let mut connect = 0;
        let rc = unsafe { AdsConnect101(s.as_mut_ptr(), std::ptr::null_mut(), &mut connect) };
        assert_eq!(rc, REMOTE_LAYER_UNAVAILABLE);
        assert_eq!(connect, 0);
    }

    #[test]
    fn find_tables() {
        let _guard = CONNECT.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ace32-find-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub.adt")).unwrap();
        for name in ["b.ADT", "a.adt", "a.adi", "c.adm"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let connect = connect_to(dir.to_str().unwrap());
        let adt = c"*.adt".as_ptr().cast_mut().cast();
        let mut name = [0u8; 16];
        let mut find = 0;

        // Both tables, in whatever order the file system has them.
        let mut len = name.len() as u16;
        let rc = unsafe { AdsFindFirstTable(connect, adt, name.as_mut_ptr(), &mut len, &mut find) };
        assert_eq!(rc, AE_SUCCESS);
        assert_ne!(find, NO_FIND);
        let mut found = vec![name[..len as usize].to_vec()];
        let mut len = name.len() as u16;
        let rc = unsafe { AdsFindNextTable(connect, find, name.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_SUCCESS);
        found.push(name[..len as usize].to_vec());
        found.sort();
        assert_eq!(found, [b"a.adt".to_vec(), b"b.ADT".to_vec()]);
        let mut len = name.len() as u16;
        let rc = unsafe { AdsFindNextTable(connect, find, name.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_NO_FILE_FOUND);
        assert_eq!(AdsFindClose(connect, find), AE_SUCCESS);
        assert_eq!(AdsFindClose(connect, find), AE_SUCCESS);

        // A name that does not fit reports its size with the NUL and is
        // skipped; the listing is open regardless. 5 bytes hold "a.adt"
        // but not its NUL.
        let mut small = [0u8; 5];
        let mut len = small.len() as u16;
        let rc =
            unsafe { AdsFindFirstTable(connect, adt, small.as_mut_ptr(), &mut len, &mut find) };
        assert_eq!(rc, AE_INSUFFICIENT_BUFFER);
        assert_eq!(len, 6);
        let mut len = name.len() as u16;
        let rc = unsafe { AdsFindNextTable(connect, find, name.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_SUCCESS);
        let mut len = name.len() as u16;
        let rc = unsafe { AdsFindNextTable(connect, find, name.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_NO_FILE_FOUND);
        assert_eq!(AdsFindClose(connect, find), AE_SUCCESS);

        // No match, including the empty mask: -1.
        for mask in [c"*.dbf", c""] {
            let mut len = name.len() as u16;
            let mask = mask.as_ptr().cast_mut().cast();
            let rc =
                unsafe { AdsFindFirstTable(connect, mask, name.as_mut_ptr(), &mut len, &mut find) };
            assert_eq!(rc, AE_NO_FILE_FOUND);
            assert_eq!(find, NO_FIND);
        }

        // A null mask succeeds without writing anything.
        let (mut len, mut find) = (7, 7);
        let null = std::ptr::null_mut();
        let rc =
            unsafe { AdsFindFirstTable(connect, null, name.as_mut_ptr(), &mut len, &mut find) };
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!((len, find), (7, 7));

        // An unknown handle.
        let mut len = name.len() as u16;
        let rc = unsafe { AdsFindNextTable(connect, 0, name.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_INVALID_HANDLE);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Connecting by path, as the original engine does: on the servers
    /// `AdsSetServerType` allows (remote and Internet ones until set) or
    /// those given.
    #[test]
    fn connecting_by_path() {
        let _settings = crate::fixtures::settings_lock();
        let dir = crate::fixtures::dir("all_types");
        let mut path = format!("{}\0", dir.display()).into_bytes();
        let path = path.as_mut_ptr();
        // Connects with `connect`; closes the connection it opens (only
        // that one: other tests run alongside).
        let result = |connect: &dyn Fn(*mut AdsHandle) -> Unsigned32| {
            let mut handle = 0;
            let rc = connect(&mut handle);
            if rc == AE_SUCCESS {
                assert!(
                    LOCAL
                        .connection_info(handle)
                        .is_ok_and(|c| Path::new(&c.path) == dir)
                );
                assert_eq!(crate::AdsDisconnect(handle), crate::AE_SUCCESS);
            }
            rc
        };
        crate::AdsSetServerType(0);
        assert_eq!(
            result(&|h| unsafe { AdsConnect(path, h) }),
            crate::REMOTE_SERVER_NOT_FOUND
        );
        assert_eq!(crate::AdsSetServerType(ADS_LOCAL_SERVER), AE_SUCCESS);
        assert_eq!(result(&|h| unsafe { AdsConnect(path, h) }), AE_SUCCESS);
        for (types, rc) in [
            (0, AE_SUCCESS),
            (1, AE_SUCCESS),
            (2, crate::REMOTE_SERVER_NOT_FOUND),
        ] {
            assert_eq!(
                result(&|h| unsafe { AdsConnect26(path, types, h) }),
                rc,
                "{types}"
            );
        }
        let rc = result(&|h| unsafe { AdsConnect26(path, 4, h) });
        assert_eq!(
            rc,
            crate::AE_NO_DRIVE_CONNECTION,
            "an Internet server alone"
        );
        let null = std::ptr::null_mut();
        for options in [0x1081, 0x10, 0x4, 0x8, 0xC] {
            let rc = result(&|h| unsafe { AdsConnect60(path, 7, null, null, options, h) });
            assert_eq!(rc, AE_SUCCESS, "{options:#x}");
        }
        let rc = result(&|h| unsafe { AdsConnect60(path, 1, null, null, 0xFFFF, h) });
        assert_eq!(rc, AE_INVALID_OPTION);
        assert_eq!(
            result(&|h| unsafe { AdsConnect(null, h) }),
            AE_INVALID_POINTER
        );
        crate::AdsSetServerType(0);
    }

    #[test]
    fn finding_tables_with_dictionary_names() {
        let connect = crate::fixtures::connect("indexes");
        let (mut dd, mut name) = ([0xEEu8; 8], [0u8; 32]);
        let (mut dd_len, mut name_len) = (dd.len() as u16, name.len() as u16);
        let mut find = 0;
        let mask = c"*.adt".as_ptr().cast_mut().cast();
        let rc = unsafe {
            AdsFindFirstTable62(
                connect,
                mask,
                dd.as_mut_ptr(),
                &mut dd_len,
                name.as_mut_ptr(),
                &mut name_len,
                &mut find,
            )
        };
        assert_eq!(
            (rc, dd_len, dd[0]),
            (AE_SUCCESS, 0, 0),
            "no linked dictionary"
        );
        assert_eq!(&name[..name_len as usize], b"indexed.adt");
        (dd_len, name_len) = (dd.len() as u16, name.len() as u16);
        let rc = unsafe {
            AdsFindNextTable62(
                connect,
                find,
                dd.as_mut_ptr(),
                &mut dd_len,
                name.as_mut_ptr(),
                &mut name_len,
            )
        };
        assert_eq!(
            (rc, &name[..name_len as usize]),
            (AE_SUCCESS, &b"pages8k.adt"[..])
        );
        let rc = unsafe {
            AdsFindNextTable62(
                connect,
                find,
                dd.as_mut_ptr(),
                &mut dd_len,
                name.as_mut_ptr(),
                &mut name_len,
            )
        };
        assert_eq!(rc, AE_NO_FILE_FOUND);
        assert_eq!(AdsFindClose(connect, find), AE_SUCCESS);
        let rc = unsafe {
            AdsFindNextTable62(
                connect,
                0x12345,
                dd.as_mut_ptr(),
                &mut dd_len,
                name.as_mut_ptr(),
                &mut name_len,
            )
        };
        assert_eq!(rc, AE_NO_FILE_FOUND, "an unknown listing");
        let null = std::ptr::null_mut();
        let rc = unsafe {
            AdsFindFirstTable62(
                connect,
                mask,
                null,
                &mut dd_len,
                name.as_mut_ptr(),
                &mut name_len,
                &mut find,
            )
        };
        assert_eq!(rc, AE_INVALID_POINTER);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    /// Connects to `path` with `AdsConnect60` on `types`.
    fn connect60(path: &str, types: Unsigned16) -> (Unsigned32, AdsHandle) {
        let mut path = format!("{path}\0").into_bytes();
        let (mut connect, null) = (0, std::ptr::null_mut());
        let rc = unsafe { AdsConnect60(path.as_mut_ptr(), types, null, null, 0, &mut connect) };
        (rc, connect)
    }

    /// The `Compression` option and `ADS_COMPRESS_*`: values checked as the
    /// original engine checks them (probed, `tools/ace/compress`), and a
    /// connection to the test server (which says always) compressed unless
    /// the client says never.
    #[test]
    fn compression_settings() {
        let connect101 = |text: &str| {
            let mut s = format!("{text}\0").into_bytes();
            let mut connect = 0;
            let rc = unsafe { AdsConnect101(s.as_mut_ptr(), std::ptr::null_mut(), &mut connect) };
            (rc, connect)
        };
        let last_error = || {
            let (mut code, mut buf, mut len) = (0, [0u8; 200], 200u16);
            unsafe { crate::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
            String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned()
        };
        let local = r"Data Source=C:\data\;ServerType=LOCAL;";
        for good in ["ALWAYS", "always", "Never", "INTERNET", "internet "] {
            let (rc, connect) = connect101(&format!("{local}Compression={good};"));
            assert_eq!(rc, AE_SUCCESS, "{good}");
            assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
        }
        for bad in ["BOGUS", "1", "TRUE", "ON", "OFF", "NONE"] {
            assert_eq!(
                connect101(&format!("{local}Compression={bad};")).0,
                AE_INVALID_OPTION
            );
            assert!(
                last_error().ends_with(&format!("specified.  {bad}")),
                "{}",
                last_error()
            );
        }
        assert_eq!(
            connect101(&format!("{local}Compression=;")).0,
            AE_INVALID_OPTION
        );
        // Refused before the server is tried.
        let nowhere = r"Data Source=\\127.0.0.1:1\x;ServerType=REMOTE;Compression=BOGUS;";
        assert_eq!(connect101(nowhere).0, AE_INVALID_OPTION);

        let port = crate::fixtures::server();
        let source = format!(r"\\127.0.0.1:{port}\fixtures\all_types");
        let compressed = |connect| match backend::api(connect) {
            Backend::Remote(remote) => remote.client.compressed(),
            Backend::Local | Backend::Foreign => panic!("not remote"),
        };
        let remote = format!("Data Source={source};ServerType=REMOTE;");
        for (option, expected) in [("", true), ("Compression=Never;", false)] {
            let (rc, connect) = connect101(&format!("{remote}{option}"));
            assert_eq!(rc, AE_SUCCESS);
            assert_eq!(compressed(connect), expected, "{option}");
            assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
        }
        let mut path = format!("{source}\0").into_bytes();
        for (options, expected) in [(0x4, true), (0x8, false), (0xC, true)] {
            let (mut connect, null) = (0, std::ptr::null_mut());
            let rc = unsafe {
                AdsConnect60(
                    path.as_mut_ptr(),
                    ADS_REMOTE_SERVER,
                    null,
                    null,
                    options,
                    &mut connect,
                )
            };
            assert_eq!(rc, AE_SUCCESS);
            assert_eq!(compressed(connect), expected, "{options:#x}");
            // The server's setting, as the original server reports its own.
            let (mut params, mut memory) = ([0u8; 864], [0u8; 56]);
            let (mut params_size, mut memory_size) = (864u16, 56u16);
            let rc = unsafe {
                crate::AdsMgGetConfigInfo(
                    connect,
                    params.as_mut_ptr(),
                    &mut params_size,
                    memory.as_mut_ptr(),
                    &mut memory_size,
                )
            };
            assert_eq!((rc, &params[0x358..0x35a]), (AE_SUCCESS, &[4, 0][..]));
            assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
        }
    }

    #[test]
    fn connecting_to_a_server() {
        let port = crate::fixtures::server();
        let source = format!(r"\\127.0.0.1:{port}\fixtures\all_types");
        let (rc, connect) = connect60(&source, ADS_REMOTE_SERVER);
        assert_eq!(rc, AE_SUCCESS);
        assert!(
            connect >= crate::backend::REMOTE_BASE,
            "a handle of the server's session"
        );
        let mut server_type = 0;
        assert_eq!(
            unsafe { AdsGetConnectionType(connect, &mut server_type) },
            AE_SUCCESS
        );
        assert_eq!(server_type, ADS_REMOTE_SERVER);
        let mut buf = [0u8; 64];
        let mut len = buf.len() as u16;
        let rc = unsafe { AdsGetConnectionPath(connect, buf.as_mut_ptr(), &mut len) };
        assert_eq!(
            (rc, &buf[..usize::from(len)]),
            (AE_SUCCESS, source.as_bytes())
        );

        // Clients such as ARC list and open tables by their full path.
        let mut mask = format!("{source}\\*.adt\0").into_bytes();
        let (mut name, mut find) = ([0u8; 32], 0);
        let mut len = name.len() as u16;
        let rc = unsafe {
            AdsFindFirstTable(
                connect,
                mask.as_mut_ptr(),
                name.as_mut_ptr(),
                &mut len,
                &mut find,
            )
        };
        assert_eq!(
            (rc, &name[..usize::from(len)]),
            (AE_SUCCESS, &b"all_types.adt"[..])
        );
        let mut full = format!("{source}\\ALL_TYPES.ADT\0").into_bytes();
        let (mut table, null) = (0, std::ptr::null_mut());
        let rc = unsafe {
            crate::AdsOpenTable(
                connect,
                full.as_mut_ptr(),
                null,
                crate::ADS_ADT,
                1,
                1,
                1,
                0,
                &mut table,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        let mut outside = format!("{source}\\..\\indexes\\indexed.adt\0").into_bytes();
        let rc = unsafe {
            crate::AdsOpenTable(
                connect,
                outside.as_mut_ptr(),
                null,
                crate::ADS_ADT,
                1,
                1,
                1,
                0,
                &mut table,
            )
        };
        assert_eq!(rc, crate::AE_FILE_NOT_ON_SERVER);
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
        let rc = unsafe { AdsGetConnectionType(connect, &mut server_type) };
        assert_eq!(rc, AE_INVALID_CONNECTION_HANDLE);

        // The server refuses what is not in its shares; with the local
        // server allowed too, the client falls back to it.
        let elsewhere = format!(r"\\127.0.0.1:{port}\nope");
        assert_eq!(
            connect60(&elsewhere, ADS_REMOTE_SERVER).0,
            crate::AE_FILE_NOT_ON_SERVER
        );
        let (rc, connect) = connect60(&elsewhere, ADS_REMOTE_SERVER | ADS_LOCAL_SERVER);
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!(
            unsafe { AdsGetConnectionType(connect, &mut server_type) },
            AE_SUCCESS
        );
        assert_eq!(server_type, ADS_LOCAL_SERVER);
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);

        // No server there.
        let nobody = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let missing = format!(r"\\127.0.0.1:{nobody}\fixtures");
        assert_eq!(
            connect60(&missing, ADS_REMOTE_SERVER).0,
            crate::REMOTE_SERVER_NOT_FOUND
        );

        // A connect string.
        let mut s = format!("Data Source={source};ServerType=REMOTE;\0").into_bytes();
        let mut connect = 0;
        let rc = unsafe { AdsConnect101(s.as_mut_ptr(), std::ptr::null_mut(), &mut connect) };
        assert_eq!(rc, AE_SUCCESS);
        assert!(connect >= crate::backend::REMOTE_BASE);
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
    }

    /// As the original engine reports them for its local server.
    #[test]
    fn server_names() {
        let name_of = |connect: AdsHandle, size: u16| {
            let mut buf = [0xEEu8; 32];
            let mut len = size;
            let rc = unsafe { AdsGetServerName(connect, buf.as_mut_ptr(), &mut len) };
            (rc, len, buf[..usize::from(size) + 1].to_vec())
        };
        for (path, server) in [(r"C:\data\", "C:"), (r"\\somehost\share\dir", "somehost")] {
            let (rc, connect) = connect60(path, ADS_LOCAL_SERVER);
            assert_eq!(rc, AE_SUCCESS);
            let (rc, len, buf) = name_of(connect, 20);
            assert_eq!(
                (rc, &buf[..usize::from(len) + 1]),
                (AE_SUCCESS, &[server.as_bytes(), &[0]].concat()[..])
            );
            assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
        }
        let (_, connect) = connect60(r"C:\data", ADS_LOCAL_SERVER);
        // Too small: what fits, without a NUL, and the length with the NUL.
        assert_eq!(
            name_of(connect, 2),
            (AE_INSUFFICIENT_BUFFER, 3, b"C:\xEE".to_vec())
        );
        assert_eq!(name_of(connect, 0), (AE_INSUFFICIENT_BUFFER, 3, vec![0xEE]));
        let mut len = 20;
        let rc = unsafe { AdsGetServerName(connect, std::ptr::null_mut(), &mut len) };
        assert_eq!((rc, len), (AE_INVALID_POINTER, 20));
        assert_eq!(name_of(0, 20).0, AE_INVALID_CONNECTION_HANDLE);
        assert_eq!(name_of(0x12345, 20).0, AE_INVALID_CONNECTION_HANDLE);
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);

        // A connection to tflipd: the server's host.
        let port = crate::fixtures::server();
        let (rc, connect) = connect60(
            &format!(r"\\127.0.0.1:{port}\fixtures\all_types"),
            ADS_REMOTE_SERVER,
        );
        assert_eq!(rc, AE_SUCCESS);
        let (rc, len, buf) = name_of(connect, 20);
        assert_eq!(
            (rc, &buf[..usize::from(len)]),
            (AE_SUCCESS, &b"127.0.0.1"[..])
        );
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
    }

    /// As the original engine answers.
    #[test]
    fn connection_properties() {
        let remote = crate::fixtures::is_remote();
        let types = if remote {
            ADS_REMOTE_SERVER
        } else {
            ADS_LOCAL_SERVER
        };
        let mut path = format!("{}\0", crate::fixtures::source("all_types")).into_bytes();
        let (mut user, mut password) = (*b"someone\0", *b"secret\0");
        let mut connect = 0;
        let rc = unsafe {
            AdsConnect60(
                path.as_mut_ptr(),
                types,
                user.as_mut_ptr(),
                password.as_mut_ptr(),
                0,
                &mut connect,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        let get = |connect, property, size: u32| {
            let mut buf = [0xEEu8; 64];
            let mut len = size;
            let rc = unsafe {
                AdsGetConnectionProperty(connect, property, buf.as_mut_ptr().cast(), &mut len)
            };
            (rc, len, buf)
        };
        let text = |property| {
            let (rc, len, buf) = get(connect, property, 64);
            assert_eq!((rc, buf[len as usize]), (AE_SUCCESS, 0), "{property}");
            String::from_utf8(buf[..len as usize].to_vec()).unwrap()
        };
        assert_eq!(text(ADS_CONNECTIONPROP_USERNAME), "someone");
        assert_eq!(text(ADS_CONNECTIONPROP_PASSWORD), "secret");
        assert_eq!(text(ADS_CONNECTIONPROP_ENCRYPTION_TYPE), "RC4");
        assert_eq!(text(ADS_CONNECTIONPROP_COMMON_NAME), "");
        assert_eq!(
            text(ADS_CONNECTIONPROP_CIPHER_SUITE).starts_with("TLS"),
            remote
        );
        let (rc, len, buf) = get(connect, ADS_CONNECTIONPROP_PROTOCOL, 64);
        let protocol = if remote {
            ADS_CONNECTIONPROP_USING_TLC
        } else {
            0
        };
        assert_eq!(
            (rc, len, &buf[..4]),
            (AE_SUCCESS, 4, &protocol.to_le_bytes()[..])
        );
        assert_eq!(get(connect, ADS_CONNECTIONPROP_FIPS_MODE, 64).1, 2);
        // Too small: text gets what fits, numbers nothing.
        let (rc, len, buf) = get(connect, ADS_CONNECTIONPROP_USERNAME, 3);
        assert_eq!(
            (rc, len, &buf[..4]),
            (AE_INSUFFICIENT_BUFFER, 8, &b"som\xEE"[..])
        );
        let (rc, len, _) = get(connect, ADS_CONNECTIONPROP_PROTOCOL, 3);
        assert_eq!((rc, len), (AE_INVALID_PROPERTY_LENGTH, 3));
        assert_eq!(get(connect, 8, 64).0, AE_INVALID_OPTION);
        assert_eq!(
            get(0, ADS_CONNECTIONPROP_USERNAME, 64).0,
            AE_INVALID_CONNECTION_HANDLE
        );
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);

        // Without credentials: empty.
        let connect = crate::fixtures::connect("all_types");
        assert_eq!(get(connect, ADS_CONNECTIONPROP_USERNAME, 64).1, 0);
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
    }

    /// As the original engine answers.
    #[test]
    fn closing_cached_tables() {
        let (connect, table) = crate::fixtures::open("all_types", c"all_types.adt");
        assert_eq!(AdsCloseCachedTables(connect), AE_SUCCESS);
        let mut eof = 0xffff;
        assert_eq!(
            unsafe { crate::AdsAtEOF(table, &mut eof) },
            AE_SUCCESS,
            "still open"
        );
        assert_eq!(AdsCloseCachedTables(table), AE_INVALID_CONNECTION_HANDLE);
        assert_eq!(AdsCloseCachedTables(0), AE_INVALID_CONNECTION_HANDLE);
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
        assert_eq!(AdsCloseCachedTables(connect), AE_INVALID_CONNECTION_HANDLE);
    }

    crate::fixtures::remote_twins!(
        closing_cached_tables,
        connection_properties,
        finding_tables_with_dictionary_names
    );
}
