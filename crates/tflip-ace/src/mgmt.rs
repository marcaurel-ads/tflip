//! The management API (`AdsMg*`), as far as clients use it on a connection.

use tflip_protocol::wire::Platform;

use crate::backend::Backend;
use crate::ffi::{c_str, text};
use crate::last_error::{Call, fail, fail_detail, null_param};

use tflip_engine::table::TableType;

use crate::{
    AE_INSUFFICIENT_BUFFER, AE_INVALID_PATH, AE_INVALID_USERNAME, AE_SUCCESS, AdsHandle,
    USER_NOT_FOUND, Unsigned8, Unsigned16, Unsigned32, ansi, error_code,
};

pub const ADS_MGMT_NT_SERVER: Unsigned16 = 2;
pub const ADS_MGMT_LOCAL_SERVER: Unsigned16 = 3;
pub const ADS_MGMT_LINUX_SERVER: Unsigned16 = 6;
pub const ADS_MGMT_NT_SERVER_64_BIT: Unsigned16 = 7;
pub const ADS_MGMT_LINUX_SERVER_64_BIT: Unsigned16 = 8;

/// The `ADS_MGMT_*_SERVER` type of a server on `platform`. Platforms ACE
/// has no type for count as Linux.
fn server_type(platform: &Platform) -> Unsigned16 {
    match (platform.os.as_str(), platform.bits) {
        ("windows", 64) => ADS_MGMT_NT_SERVER_64_BIT,
        ("windows", _) => ADS_MGMT_NT_SERVER,
        (_, 64) => ADS_MGMT_LINUX_SERVER_64_BIT,
        _ => ADS_MGMT_LINUX_SERVER,
    }
}

/// `AdsMgGetServerType`: the kind of server behind a connection, as one of
/// the `ADS_MGMT_*_SERVER` constants: `ADS_MGMT_LOCAL_SERVER` for the local
/// server, as in the original engine, and for `tflipd` the type of the
/// platform it runs on (`ADS_MGMT_LINUX_SERVER_64_BIT`, ...).
///
/// Management handles (`AdsMgConnect`) do not exist yet; a connection
/// handle is taken in their place, as the original engine takes it. As
/// there, any other handle fails with `AE_INVALID_CONNECTION_HANDLE`, with
/// `AE_INVALID_HANDLE` as the last error for a value that is no handle.
///
/// # Safety
///
/// `server_type_ptr` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsMgGetServerType(
    mgmt: AdsHandle,
    server_type_ptr: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsMgGetServerType");
    let backend = match crate::connection::checked(mgmt) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    if server_type_ptr.is_null() {
        return null_param(2);
    }
    let value = match &backend {
        Backend::Remote(remote) => server_type(remote.client.platform()),
        Backend::Local | Backend::Foreign => ADS_MGMT_LOCAL_SERVER,
    };
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *server_type_ptr = value };
    AE_SUCCESS
}

/// The size of `ADS_MGMT_ACTIVITY_INFO` (packed): two counters, the uptime
/// (four `UNSIGNED16`) and 13 `ADS_MGMT_USAGE_INFO` (three `UNSIGNED32`).
const ACTIVITY_INFO_SIZE: usize = 4 + 4 + 8 + 13 * 12;

/// `AdsMgGetActivityInfo`: what the server behind a connection has in use,
/// as an `ADS_MGMT_ACTIVITY_INFO` in `info` of `*size` bytes; `*size` is set
/// to the structure's size (172 bytes). As in the original engine, a
/// smaller buffer gets the bytes that fit, and a size of 0 fails with
/// `AE_INSUFFICIENT_BUFFER` (with `*size` set to 0).
///
/// The counts are the session's: connections, work areas (open table
/// handles), tables (distinct files) and indexes (distinct index files) in
/// use; for `tflipd`, those of the connection's session on the server. The
/// "max used" of each is its count in use (0 for connections, as in the
/// original engine), and everything else is 0 (see docs/KNOWN_ISSUES.md). The handle is taken as for `AdsMgGetServerType`.
///
/// # Safety
///
/// `info` must be null or hold `*size` bytes; `size` must be null or valid
/// for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsMgGetActivityInfo(
    mgmt: AdsHandle,
    info: *mut Unsigned8,
    size: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsMgGetActivityInfo");
    let backend = match crate::connection::checked(mgmt) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    let activity = match backend.activity() {
        Ok(activity) => activity,
        Err(error) => return error_code(error),
    };
    let mut bytes = Vec::with_capacity(ACTIVITY_INFO_SIZE);
    // Operations and logged errors, and the uptime.
    bytes.extend_from_slice(&[0; 4 + 4 + 8]);
    // In use, max used and rejected, for: users, connections, work areas,
    // tables, indexes, locks, the three TPS elements, worker threads,
    // queries, stateful and stateless users.
    let in_use = [
        0,
        activity.connections,
        activity.work_areas,
        activity.tables,
        activity.indexes,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    for (i, count) in in_use.into_iter().enumerate() {
        // The original engine's local server reports no maximum of
        // connections.
        let max_used = if i == 1 { 0 } else { count };
        for value in [count, max_used, 0u32] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    // SAFETY: `info` and `size` are as # Safety says.
    unsafe { write_struct(&bytes, info, size, 2) }
}

/// Fails as the original engine does with the buffer `out` and its size
/// `size`, arguments `out_param` and `out_param + 1`: a NULL size first; a
/// size of 0 is `AE_INSUFFICIENT_BUFFER`, which the original names the
/// size's argument for, and is looked at before a NULL buffer, which a size
/// of anything else makes `AE_INVALID_POINTER`.
///
/// # Safety
///
/// `size` must be null or valid for reads.
unsafe fn check_struct(
    out: *mut Unsigned8,
    size: *mut Unsigned16,
    out_param: u32,
) -> Result<(), Unsigned32> {
    if size.is_null() {
        return Err(null_param(out_param + 1));
    }
    // SAFETY: not null, so readable, as the caller promises.
    if unsafe { *size } == 0 {
        return Err(insufficient(out_param + 1));
    }
    if out.is_null() {
        return Err(null_param(out_param));
    }
    Ok(())
}

/// `AE_INSUFFICIENT_BUFFER` for the argument `param`.
fn insufficient(param: u32) -> Unsigned32 {
    fail_detail(AE_INSUFFICIENT_BUFFER, &format!("Parameter {param}"))
}

/// Writes `bytes`, a management structure, to `out` of `*size` bytes (the
/// arguments `out_param` and the one after), as the original engine does:
/// as much as fits, with `*size` set to the structure's size; a size of 0
/// fails with `AE_INSUFFICIENT_BUFFER` (see [`check_struct`]).
///
/// # Safety
///
/// `out` must be null or hold `*size` bytes; `size` must be null or valid
/// for reads and writes.
unsafe fn write_struct(
    bytes: &[u8],
    out: *mut Unsigned8,
    size: *mut Unsigned16,
    out_param: u32,
) -> Unsigned32 {
    // SAFETY: `size` is as # Safety says.
    if let Err(code) = unsafe { check_struct(out, size, out_param) } {
        return code;
    }
    // SAFETY: not null, so readable, as the caller promises.
    let room = unsafe { *size };
    let fits = bytes.len().min(usize::from(room));
    // SAFETY: `out` holds `*size` bytes, and `size` is writable, as the
    // caller promises.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, fits);
        *size = bytes.len() as Unsigned16;
    }
    AE_SUCCESS
}

/// The size of `ADS_MGMT_COMM_STATS` (packed): a double and ten counters.
const COMM_STATS_SIZE: usize = 8 + 10 * 4;

/// `AdsMgGetCommStats`: the server's communication statistics (packets,
/// checksum failures, ...) as an `ADS_MGMT_COMM_STATS` in `stats` of
/// `*size` bytes; `*size` is set to the structure's size (48 bytes). There
/// are none to report: all are 0, as the original engine's local server
/// reports them, and for `tflipd`, which talks over TCP. The size and the
/// handle are taken as for `AdsMgGetActivityInfo`.
///
/// # Safety
///
/// `stats` must be null or hold `*size` bytes; `size` must be null or valid
/// for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsMgGetCommStats(
    mgmt: AdsHandle,
    stats: *mut Unsigned8,
    size: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsMgGetCommStats");
    if let Err(code) = crate::connection::checked(mgmt) {
        return code;
    }
    // SAFETY: `stats` and `size` are as # Safety says.
    unsafe { write_struct(&[0; COMM_STATS_SIZE], stats, size, 2) }
}

/// The size of `ADS_MGMT_USER_INFO` (packed): user name, connection
/// number, dictionary user name, address, OS login name, terminal services
/// address, application ID, average request cost and a reserved field.
const USER_INFO_SIZE: usize = 50 + 2 + 50 + 30 + 50 + 30 + 70 + 4 + 2;

/// An `ADS_MGMT_USER_INFO` for a user of this process, as the original
/// engine's local server fills it in: the OS user's name and the
/// application ID (the program's file name); the rest is empty.
fn user_info() -> Vec<u8> {
    let mut info = vec![0; USER_INFO_SIZE];
    let mut put = |at: usize, size: usize, text: &str| {
        let bytes = ansi::from_str_lossy(text);
        let n = bytes.len().min(size - 1);
        info[at..at + n].copy_from_slice(&bytes[..n]);
    };
    put(0, 50, &sys::user_name());
    let program = std::env::current_exe().ok();
    let program = program
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy());
    put(212, 70, program.as_deref().unwrap_or_default());
    info
}

/// `AdsMgGetUserNames`: the users connected to the server, as an array of
/// `ADS_MGMT_USER_INFO` in `users` of `*len` entries `*size` bytes apart; with
/// `file`, only those with it open. `*len` is set to the number written,
/// `*size` to the structure's size (288 bytes).
///
/// As the original engine's local server reports them, each connection is a
/// user, with the OS user's name and the program's file name as its
/// application ID; for `tflipd`, the connections of the connection's session
/// on the server. As there: entries that do not fit are left out (no
/// error), a length or size of 0 fails with `AE_INSUFFICIENT_BUFFER`, a
/// smaller size cuts each entry and a larger one pads it with zeros, an
/// empty `file` fails with
/// `AE_INVALID_PATH` and one no connection has open with 7051 (not in
/// `ace.h`). A relative `file`, and for `tflipd` one inside the connection's
/// server path, is taken relative to the connection's folder. The handle is
/// taken as for `AdsMgGetServerType`.
///
/// # Safety
///
/// `file` must be null or point to a NUL-terminated string; `users` must be
/// null or hold `*len` entries of `*size` bytes; `len` and `size` must be
/// null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsMgGetUserNames(
    mgmt: AdsHandle,
    file: *mut Unsigned8,
    users: *mut Unsigned8,
    len: *mut Unsigned16,
    size: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsMgGetUserNames");
    // SAFETY: `file` is null or NUL-terminated, as # Safety says.
    let file = unsafe { text(file) };
    let backend = match crate::connection::checked(mgmt) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    // SAFETY: `len` and `size` are null or readable, as # Safety says.
    let room = match unsafe { check_entries(users, len, size, 3) } {
        Ok(room) => room,
        Err(code) => return code,
    };
    let file = file.map(|name| crate::remote::relative_name(mgmt, &name));
    let count = match backend.users(file) {
        Ok(count) => usize::try_from(count).unwrap_or(usize::MAX),
        Err(error) => {
            // As in the original engine, a file no one has open lists no one.
            if error == tflip_engine::Error::FileNotOpen {
                // SAFETY: checked not null, so writable, as # Safety says.
                unsafe { *len = 0 };
            }
            return error_code(error);
        }
    };
    let entries = vec![user_info(); count.min(room)];
    // SAFETY: `users`, `len` and `size` are as # Safety says.
    unsafe { write_entries(&entries, USER_INFO_SIZE, users, len, size) }
}

/// Checks the array arguments of a management function that fills an
/// array, `out` being argument `out_param` and `len` and `size` the next
/// two, as the original engine does (see also [`check_entry_room`], which a
/// function with an argument of its own to check calls after this): the
/// pointers that are not null, in the order of the arguments.
///
/// # Safety
///
/// Nothing is read.
fn check_entry_pointers(
    out: *mut Unsigned8,
    len: *mut Unsigned16,
    size: *mut Unsigned16,
    out_param: u32,
) -> Result<(), Unsigned32> {
    for (i, null) in [out.is_null(), len.is_null(), size.is_null()]
        .into_iter()
        .enumerate()
    {
        if null {
            return Err(null_param(out_param + i as u32));
        }
    }
    Ok(())
}

/// Checks the length and entry size of an array: not 0
/// (`AE_INSUFFICIENT_BUFFER`, named for the size's argument first, as in
/// the original engine). Returns the length, the entries there is room for.
///
/// # Safety
///
/// `len` and `size` must be valid for reads.
unsafe fn check_entry_room(
    len: *mut Unsigned16,
    size: *mut Unsigned16,
    out_param: u32,
) -> Result<usize, Unsigned32> {
    // SAFETY: readable, as the caller promises.
    let (room, stride) = unsafe { (*len, *size) };
    if stride == 0 {
        return Err(insufficient(out_param + 2));
    }
    if room == 0 {
        return Err(insufficient(out_param + 1));
    }
    Ok(usize::from(room))
}

/// [`check_entry_pointers`] and then [`check_entry_room`].
///
/// # Safety
///
/// `len` and `size` must be null or valid for reads.
unsafe fn check_entries(
    out: *mut Unsigned8,
    len: *mut Unsigned16,
    size: *mut Unsigned16,
    out_param: u32,
) -> Result<usize, Unsigned32> {
    check_entry_pointers(out, len, size, out_param)?;
    // SAFETY: not null, so readable, as the caller promises.
    unsafe { check_entry_room(len, size, out_param) }
}

/// Writes `entries`, structures of one size, to the array `out` of `*len`
/// entries `*size` bytes apart, as the original engine does: the entries
/// that fit, each cut or padded with zeros to `*size` bytes; `*len` is set
/// to the number written and `*size` to the structures' size, `entry_size`.
///
/// # Safety
///
/// As checked by [`check_entries`]; `out` must hold `*len` entries of
/// `*size` bytes.
unsafe fn write_entries(
    entries: &[Vec<u8>],
    entry_size: usize,
    out: *mut Unsigned8,
    len: *mut Unsigned16,
    size: *mut Unsigned16,
) -> Unsigned32 {
    // SAFETY: checked not null, so readable, as the caller promises.
    let (room, stride) = unsafe { (usize::from(*len), usize::from(*size)) };
    let written = entries.len().min(room);
    for (i, entry) in entries.iter().take(written).enumerate() {
        let mut bytes = entry.clone();
        bytes.resize(stride, 0);
        // SAFETY: entry `i` is one of the `*len` entries of `*size` bytes
        // `out` holds, as the caller promises.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out.add(i * stride), stride) };
    }
    // SAFETY: checked not null, so writable, as the caller promises.
    unsafe {
        *len = written as Unsigned16;
        *size = entry_size as Unsigned16;
    }
    AE_SUCCESS
}

/// Checks the user a management function is asked about, and returns
/// whether one was named: as the original engine's local server, the user
/// is the OS user the process runs as. Another user, or a NetWare
/// connection number other than 0, fails with 7050 (not in `ace.h`) and
/// sets `*len` to 0; an empty name fails with `AE_INVALID_USERNAME`.
fn check_user(
    user: Option<&[u8]>,
    connection_number: Unsigned16,
    len: &mut Unsigned16,
) -> Result<bool, Unsigned32> {
    if user == Some(b"") {
        return Err(fail(AE_INVALID_USERNAME));
    }
    let ours = |name: &[u8]| ansi::to_string(name).eq_ignore_ascii_case(&sys::user_name());
    if connection_number != 0 || user.is_some_and(|name| !ours(name)) {
        *len = 0;
        return Err(fail(USER_NOT_FOUND));
    }
    Ok(user.is_some())
}

/// The size of `ADS_MGMT_TABLE_INFO` (packed): the table's name and its
/// locking mode.
const TABLE_INFO_SIZE: usize = ADS_MGMT_MAX_PATH + 2;

/// `ADS_MGMT_MAX_PATH`, the room for a table's name, its NUL included.
const ADS_MGMT_MAX_PATH: usize = 260;

pub const ADS_MGMT_CDX_LOCKING: Unsigned16 = 2;
pub const ADS_MGMT_NTX_LOCKING: Unsigned16 = 3;
pub const ADS_MGMT_ADT_LOCKING: Unsigned16 = 4;

/// `AdsMgGetOpenTables`: the tables open on the server, as an array of
/// `ADS_MGMT_TABLE_INFO` (the table's file and its `ADS_MGMT_*_LOCKING`
/// mode) in `tables` of `*len` entries `*size` bytes apart. Without a user
/// name, each file open on the server once; with one, every table its
/// connections have open, the same file as often as it is open. `*len` is
/// set to the number written, `*size` to the structure's size (262 bytes).
///
/// As the original engine's local server answers: the user is the OS user
/// the process runs as (another user or a NetWare connection number other
/// than 0 fails with 7050, not in `ace.h`, an empty name with
/// `AE_INVALID_USERNAME`); the arrays are filled as by `AdsMgGetUserNames`.
/// For `tflipd`, the tables of the connection's session on the server, by
/// their paths there. The handle is taken as for `AdsMgGetServerType`.
///
/// # Safety
///
/// `user` must be null or point to a NUL-terminated string; `tables` must
/// be null or hold `*len` entries of `*size` bytes; `len` and `size` must be
/// null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsMgGetOpenTables(
    mgmt: AdsHandle,
    user: *mut Unsigned8,
    connection_number: Unsigned16,
    tables: *mut Unsigned8,
    len: *mut Unsigned16,
    size: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsMgGetOpenTables");
    // SAFETY: `user` is null or NUL-terminated, as # Safety says.
    let user = unsafe { c_str(user) };
    let backend = match crate::connection::checked(mgmt) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    // SAFETY: `len` and `size` are null or readable, as # Safety says.
    if let Err(code) = unsafe { check_entries(tables, len, size, 4) } {
        return code;
    }
    // SAFETY: checked not null, so readable and writable, as # Safety says.
    let by_user = match check_user(user, connection_number, unsafe { &mut *len }) {
        Ok(by_user) => by_user,
        Err(code) => return code,
    };
    let open = match backend.open_tables(by_user) {
        Ok(open) => open,
        Err(error) => return error_code(error),
    };
    let entries: Vec<Vec<u8>> = open
        .iter()
        .map(|table| {
            let mut entry = vec![0; TABLE_INFO_SIZE];
            let name = ansi::from_str_lossy(&table.path);
            let n = name.len().min(ADS_MGMT_MAX_PATH - 1);
            entry[..n].copy_from_slice(&name[..n]);
            let locking = match table.table_type {
                TableType::Cdx | TableType::Vfp => ADS_MGMT_CDX_LOCKING,
                TableType::Ntx => ADS_MGMT_NTX_LOCKING,
                TableType::Adt | TableType::Database => ADS_MGMT_ADT_LOCKING,
            };
            entry[ADS_MGMT_MAX_PATH..].copy_from_slice(&locking.to_le_bytes());
            entry
        })
        .collect();
    // SAFETY: `tables`, `len` and `size` are as # Safety says.
    unsafe { write_entries(&entries, TABLE_INFO_SIZE, tables, len, size) }
}

/// The size of `ADS_MGMT_CONFIG_PARAMS` (packed).
const CONFIG_PARAMS_SIZE: usize = 864;

/// The size of `ADS_MGMT_CONFIG_MEMORY` (packed): 14 `UNSIGNED32`.
const CONFIG_MEMORY_SIZE: usize = 14 * 4;

/// The `ADS_MGMT_CONFIG_PARAMS` the original engine's local server
/// reports, its defaults: 20 connections, 100 work areas, 50 tables, 75
/// indexes, 500 locks, a user buffer of 534 bytes, an error log of up to
/// 1,000 KB in `C:\`, an index sort buffer of 32 KB, a flush frequency of
/// 20,000 ms, dynamic AOFs and 100 queries; everything else 0. A server's
/// compression setting (`usCompressionLevel`, `ADS_COMPRESS_*`; 0 for the
/// local server, as the original's reports) is `compression`.
fn config_params(compression: u16) -> Vec<u8> {
    let mut params = vec![0u8; CONFIG_PARAMS_SIZE];
    let mut put = |at: usize, bytes: &[u8]| params[at..at + bytes.len()].copy_from_slice(bytes);
    for (i, value) in [20u32, 100, 50, 75, 500, 534, 0, 1000]
        .into_iter()
        .enumerate()
    {
        put(4 * i, &value.to_le_bytes());
    }
    put(0x34, &32768u32.to_le_bytes()); // index sort buffer
    put(0x38, b"C:\\"); // error log path
    put(0x344, &20000u32.to_le_bytes()); // flush frequency
    put(0x34e, &[1]); // dynamic AOFs
    put(0x358, &compression.to_le_bytes()); // compression
    put(0x35a, &100u32.to_le_bytes()); // queries
    params
}

/// Writes `bytes`, a configuration structure, to `out` of `*size` bytes,
/// as `AdsMgGetConfigInfo` does in the original engine: a smaller size
/// gets what fits and keeps its value, a larger one is filled with zeros
/// past the structure and set to its size.
///
/// # Safety
///
/// `out` must hold `*size` bytes; `size` must be valid for reads and
/// writes.
unsafe fn write_config(bytes: &[u8], out: *mut Unsigned8, size: *mut Unsigned16) {
    // SAFETY: readable, as the caller promises.
    let room = usize::from(unsafe { *size });
    let mut filled = bytes.to_vec();
    filled.resize(room, 0);
    // SAFETY: `out` holds `*size` bytes and `size` is writable, as the
    // caller promises.
    unsafe {
        std::ptr::copy_nonoverlapping(filled.as_ptr(), out, room);
        *size = room.min(bytes.len()) as Unsigned16;
    }
}

/// `AdsMgGetConfigInfo`: the server's configuration, as an
/// `ADS_MGMT_CONFIG_PARAMS` in `params` of `*params_size` bytes and an
/// `ADS_MGMT_CONFIG_MEMORY` in `memory` of `*memory_size` bytes.
///
/// The parameters are those the original engine's local server reports,
/// its defaults; tflip has no such limits. For `tflipd`, the compression
/// setting is the server's (`ADS_COMPRESS_*`, as the original server reports
/// its `COMPRESSION`); the local server's is 0. The memory the configuration
/// takes is 0 throughout: tflip allocates nothing up front (see
/// docs/KNOWN_ISSUES.md). As in the original engine: either size 0 fails
/// with `AE_INSUFFICIENT_BUFFER` and any null pointer with
/// `AE_INVALID_POINTER`, before anything is written, each naming its
/// argument (`Parameter N`) in the order `fixtures/errors/param_n_mg.json`
/// shows; a smaller size gets
/// what fits and keeps its value; a larger one is filled with zeros past
/// the structure and set to its size (864 and 56 bytes). The handle is
/// taken as for `AdsMgGetServerType`.
///
/// # Safety
///
/// `params` and `memory` must be null or hold `*params_size` and
/// `*memory_size` bytes; the sizes must be null or valid for reads and
/// writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsMgGetConfigInfo(
    mgmt: AdsHandle,
    params: *mut Unsigned8,
    params_size: *mut Unsigned16,
    memory: *mut Unsigned8,
    memory_size: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsMgGetConfigInfo");
    let compression = match crate::connection::checked(mgmt) {
        Ok(crate::backend::Backend::Remote(remote)) => remote.client.server_compression().ads(),
        Ok(_) => 0,
        Err(code) => return code,
    };
    // As the original engine does, a pair of a buffer and its size at a
    // time: the size, and the buffer unless its size is 0, and only then
    // the sizes of 0.
    // SAFETY: the sizes are null or readable, as # Safety says.
    unsafe {
        for (buffer, buffer_size, param) in [(params, params_size, 2), (memory, memory_size, 4)] {
            if buffer_size.is_null() {
                return null_param(param + 1);
            }
            if *buffer_size != 0 && buffer.is_null() {
                return null_param(param);
            }
        }
        if *params_size == 0 {
            return insufficient(3);
        }
        if *memory_size == 0 {
            return insufficient(5);
        }
    }
    let params_bytes = config_params(compression);
    // SAFETY: not null, so the buffers hold the sizes, which are readable
    // and writable, as # Safety says.
    unsafe {
        write_config(&params_bytes, params, params_size);
        write_config(&[0; CONFIG_MEMORY_SIZE], memory, memory_size);
    }
    AE_SUCCESS
}

/// `AdsMgGetOpenIndexes`: the index files open on the server, as an array of
/// `ADS_MGMT_INDEX_INFO` (the file's path) in `indexes` of `*len` entries
/// `*size` bytes apart. Without a table or user name, each file open on the
/// server once; with a user, those of every table its connections have open,
/// once per table handle; with a table (its full path, or a path relative
/// to the connection's folder), those of every handle of that table. `*len`
/// is set to the number written, `*size` to the structure's size (260
/// bytes).
///
/// As in the original engine: a table no connection has open fails with
/// 7051 (not in `ace.h`) and sets `*len` to 0, an empty table name with
/// `AE_INVALID_PATH`; the user, the arrays and the handle are taken as for
/// `AdsMgGetOpenTables`. For `tflipd`, the index files of the connection's
/// session on the server, by their paths there.
///
/// # Safety
///
/// `table` and `user` must be null or point to NUL-terminated strings;
/// `indexes` must be null or hold `*len` entries of `*size` bytes; `len` and
/// `size` must be null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsMgGetOpenIndexes(
    mgmt: AdsHandle,
    table: *mut Unsigned8,
    user: *mut Unsigned8,
    connection_number: Unsigned16,
    indexes: *mut Unsigned8,
    len: *mut Unsigned16,
    size: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsMgGetOpenIndexes");
    // SAFETY: the strings are null or NUL-terminated, as # Safety says.
    let (table, user) = unsafe { (text(table), c_str(user)) };
    let backend = match crate::connection::checked(mgmt) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    // SAFETY: `len` and `size` are null or readable, as # Safety says.
    if let Err(code) = unsafe { check_entries(indexes, len, size, 5) } {
        return code;
    }
    let table = table.map(|name| crate::remote::relative_name(mgmt, &name));
    if table.as_deref() == Some("") {
        return fail(AE_INVALID_PATH);
    }
    // SAFETY: checked not null, so readable and writable, as # Safety says.
    let len_ref = unsafe { &mut *len };
    let by_user = match check_user(user, connection_number, len_ref) {
        Ok(by_user) => by_user,
        Err(code) => return code,
    };
    let files = match backend.open_indexes(table, by_user) {
        Ok(files) => files,
        Err(error) => {
            if error == tflip_engine::Error::FileNotOpen {
                *len_ref = 0;
            }
            return error_code(error);
        }
    };
    let entries: Vec<Vec<u8>> = files
        .iter()
        .map(|path| {
            let mut entry = vec![0; ADS_MGMT_MAX_PATH];
            let name = ansi::from_str_lossy(path);
            let n = name.len().min(ADS_MGMT_MAX_PATH - 1);
            entry[..n].copy_from_slice(&name[..n]);
            entry
        })
        .collect();
    // SAFETY: `indexes`, `len` and `size` are as # Safety says.
    unsafe { write_entries(&entries, ADS_MGMT_MAX_PATH, indexes, len, size) }
}

/// The size of `ADS_MGMT_INSTALL_INFO` (packed): the user option, the
/// owner (36 bytes), six strings of 16 bytes and two user limits.
const INSTALL_INFO_SIZE: usize = 4 + 36 + 6 * 16 + 4 + 4;

/// The `ADS_MGMT_INSTALL_INFO` the original engine's local server reports,
/// byte for byte: 5 users (stateful), no owner, version `12.0.0.0`, no
/// install date, the `USA` OEM and `ENGL(AMER)` ANSI character sets, no
/// evaluation expiry, and a serial number of `@`s. The strings are padded
/// as there.
fn install_info() -> Vec<u8> {
    let mut info = vec![0u8; INSTALL_INFO_SIZE];
    let mut put = |at: usize, bytes: &[u8]| info[at..at + bytes.len()].copy_from_slice(bytes);
    put(0, &5u32.to_le_bytes()); // user option
    put(4, &[b' '; 35]); // registered owner
    put(40, b"12.0.0.0    "); // version
    put(56, &[b' '; 12]); // install date
    put(72, b"USA\0    "); // OEM character set
    put(88, b"ENGL(AMER)"); // ANSI character set
    put(120, b"@@@@@@@@@@"); // serial number
    put(136, &5u32.to_le_bytes()); // stateful users
    info
}

/// `AdsMgGetInstallInfo`: how the server is installed, as an
/// `ADS_MGMT_INSTALL_INFO` in `info` of `*size` bytes; `*size` is set to the
/// structure's size (144 bytes). The values are those the original engine's
/// local server reports (see `install_info`), for `tflipd` as well. The
/// size and the handle are taken as for `AdsMgGetActivityInfo`.
///
/// # Safety
///
/// `info` must be null or hold `*size` bytes; `size` must be null or valid
/// for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsMgGetInstallInfo(
    mgmt: AdsHandle,
    info: *mut Unsigned8,
    size: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsMgGetInstallInfo");
    if let Err(code) = crate::connection::checked(mgmt) {
        return code;
    }
    // SAFETY: `info` and `size` are as # Safety says.
    unsafe { write_struct(&install_info(), info, size, 2) }
}

#[cfg(windows)]
mod sys {
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn GetUserNameW(name: *mut u16, size: *mut u32) -> i32;
    }

    /// The name of the user the process runs as.
    pub(super) fn user_name() -> String {
        let mut name = [0u16; 257];
        let mut size = name.len() as u32;
        // SAFETY: `name` holds `size` units.
        if unsafe { GetUserNameW(name.as_mut_ptr(), &mut size) } == 0 {
            return String::new();
        }
        // `size` counts the NUL.
        String::from_utf16_lossy(&name[..(size as usize).saturating_sub(1)])
    }
}

#[cfg(not(windows))]
mod sys {
    /// The name of the user the process runs as.
    pub(super) fn user_name() -> String {
        std::env::var("USER").unwrap_or_default()
    }
}

/// The record locks of the table `file` (a path, as the server names it),
/// over every handle of this connection's process on it.
fn record_locks(backend: &Backend, file: &str) -> Vec<(AdsHandle, u32)> {
    let wanted = file.replace('/', "\\").to_ascii_lowercase();
    let mut out = Vec::new();
    for table in backend.tables(0).unwrap_or_default() {
        let Ok(info) = backend.table_info(table) else {
            continue;
        };
        let path = info.path.replace('/', "\\").to_ascii_lowercase();
        if path != wanted && !path.ends_with(&format!("\\{wanted}")) {
            continue;
        }
        for record in backend.locks(table).unwrap_or_default() {
            out.push((table, record));
        }
    }
    out
}

/// `AdsMgGetLocks`: the records locked in the table `table` (a full path),
/// as an array of `ADS_MGMT_RECORD_INFO` (a record number each); the user
/// and connection number are not looked at.
///
/// # Safety
///
/// `table` must be null or point to a NUL-terminated string; `locks` must
/// be null or hold `*len` entries of `*size` bytes; `len` and `size` must
/// be null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsMgGetLocks(
    mgmt: AdsHandle,
    table: *mut Unsigned8,
    _user: *mut Unsigned8,
    _connection: Unsigned16,
    locks: *mut Unsigned8,
    len: *mut Unsigned16,
    size: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsMgGetLocks");
    // SAFETY: `table` is null or NUL-terminated, as # Safety says.
    let table = unsafe { text(table) };
    let backend = match crate::connection::checked(mgmt) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    if let Err(code) = check_entry_pointers(locks, len, size, 5) {
        return code;
    }
    // The table is looked at after the pointers and before the sizes.
    let Some(name) = table else {
        return null_param(2);
    };
    // SAFETY: not null, so readable, as # Safety says.
    let room = match unsafe { check_entry_room(len, size, 5) } {
        Ok(room) => room,
        Err(code) => return code,
    };
    let name = crate::remote::relative_name(mgmt, &name);
    let entries: Vec<Vec<u8>> = record_locks(&backend, &name)
        .into_iter()
        .map(|(_, record)| record.to_le_bytes().to_vec())
        .collect();
    let count = entries.len().min(room);
    // SAFETY: `locks`, `len` and `size` are as # Safety says.
    unsafe { write_entries(&entries[..count], 4, locks, len, size) }
}

/// `AdsMgGetLockOwner`: who has record `record` of the table `table` (a
/// full path) locked: an `ADS_MGMT_USER_INFO` and `ADS_MGMT_RECORD_LOCK`,
/// or nothing and `ADS_MGMT_NO_LOCK` when no one has, as the original's
/// local server answers.
///
/// # Safety
///
/// `table` must be null or point to a NUL-terminated string; `info` must be
/// null or hold `*size` bytes; `size` and `lock_type` must be null or valid
/// for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsMgGetLockOwner(
    mgmt: AdsHandle,
    table: *mut Unsigned8,
    record: Unsigned32,
    info: *mut Unsigned8,
    size: *mut Unsigned16,
    lock_type: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsMgGetLockOwner");
    let backend = match crate::connection::checked(mgmt) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    // As the original engine checks them: the table, the size and whether
    // it is 0, the lock type, and last the buffer.
    if table.is_null() {
        return null_param(2);
    }
    if size.is_null() {
        return null_param(5);
    }
    // SAFETY: not null, so readable, as # Safety says.
    if unsafe { *size } == 0 {
        return insufficient(5);
    }
    if lock_type.is_null() {
        return null_param(6);
    }
    if info.is_null() {
        return null_param(4);
    }
    // SAFETY: not null, so NUL-terminated, as # Safety says.
    let name = unsafe { text(table) }.unwrap_or_default();
    let name = crate::remote::relative_name(mgmt, &name);
    let locked = record_locks(&backend, &name)
        .iter()
        .any(|&(_, r)| r == record);
    let owner = match locked {
        true => user_info(),
        false => vec![0; USER_INFO_SIZE],
    };
    let kind = if locked {
        ADS_MGMT_RECORD_LOCK
    } else {
        ADS_MGMT_NO_LOCK
    };
    // SAFETY: not null, so readable, as # Safety says.
    let n = owner.len().min(usize::from(unsafe { *size }));
    // SAFETY: not null, so `info` holds `*size` bytes and the others are
    // writable, as # Safety says.
    unsafe {
        std::ptr::copy_nonoverlapping(owner.as_ptr(), info, n);
        *size = USER_INFO_SIZE as Unsigned16;
        *lock_type = kind;
    }
    AE_SUCCESS
}

/// `ADS_MGMT_NO_LOCK` and `ADS_MGMT_RECORD_LOCK`.
const ADS_MGMT_NO_LOCK: Unsigned16 = 1;
const ADS_MGMT_RECORD_LOCK: Unsigned16 = 2;

/// The size of an `ADS_MGMT_THREAD_ACTIVITY`.
const THREAD_ACTIVITY_SIZE: Unsigned16 = 110;

/// `AdsMgGetWorkerThreadActivity`: the server's worker threads busy with an
/// operation; tflip's local server has none (`*len` 0), as the original's
/// reports.
///
/// # Safety
///
/// `threads` must be null or hold `*len` entries of `*size` bytes; `len`
/// and `size` must be null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsMgGetWorkerThreadActivity(
    mgmt: AdsHandle,
    threads: *mut Unsigned8,
    len: *mut Unsigned16,
    size: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsMgGetWorkerThreadActivity");
    if let Err(code) = crate::connection::checked(mgmt) {
        return code;
    }
    // SAFETY: `len` and `size` are null or readable, as # Safety says.
    if let Err(code) = unsafe { check_entries(threads, len, size, 2) } {
        return code;
    }
    // SAFETY: checked not null, so writable, as # Safety says.
    unsafe {
        *len = 0;
        *size = THREAD_ACTIVITY_SIZE;
    }
    AE_SUCCESS
}

/// `AdsMgDumpInternalTables`: has the server write its internal tables to
/// its log; there is nothing to write.
#[unsafe(no_mangle)]
pub extern "system" fn AdsMgDumpInternalTables(mgmt: AdsHandle) -> Unsigned32 {
    let _call = Call::begin("AdsMgDumpInternalTables");
    crate::connection::checked(mgmt).err().unwrap_or(AE_SUCCESS)
}

/// `AdsMgKillUser`: disconnects a user from the server; as the original's
/// local server, tflip's takes it and does nothing.
///
/// # Safety
///
/// `user` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsMgKillUser(
    mgmt: AdsHandle,
    user: *mut Unsigned8,
    _connection: Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsMgKillUser");
    if let Err(code) = crate::connection::checked(mgmt) {
        return code;
    }
    if user.is_null() {
        return null_param(2);
    }
    AE_SUCCESS
}

/// `AdsMgKillUser90`: [`AdsMgKillUser`] with a property, not used.
///
/// # Safety
///
/// As for [`AdsMgKillUser`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsMgKillUser90(
    mgmt: AdsHandle,
    user: *mut Unsigned8,
    _connection: Unsigned16,
    _property: Unsigned16,
    _value: *mut std::ffi::c_void,
    _value_len: Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsMgKillUser90");
    if let Err(code) = crate::connection::checked(mgmt) {
        return code;
    }
    if user.is_null() {
        return null_param(2);
    }
    AE_SUCCESS
}

/// `AdsMgConnect`: a handle for the management functions on the server
/// `server`. As the help says the original engine does it, this is
/// `AdsConnect`: the user and password are not looked at, and the handle is
/// a connection, which `AdsMgDisconnect` closes.
///
/// # Safety
///
/// `server` must be null or point to a NUL-terminated string; `handle` must
/// be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsMgConnect(
    server: *mut Unsigned8,
    _user: *mut Unsigned8,
    _password: *mut Unsigned8,
    handle: *mut AdsHandle,
) -> Unsigned32 {
    let _call = Call::begin("AdsMgConnect");
    // The original numbers the handle's argument 2, as `AdsConnect`'s.
    if server.is_null() {
        return null_param(1);
    }
    if handle.is_null() {
        return null_param(2);
    }
    // SAFETY: `server` and `handle` are as `AdsConnect26` takes them, as
    // # Safety says.
    unsafe { crate::AdsConnect26(server, 0, handle) }
}

/// `AdsMgDisconnect`: closes a handle from `AdsMgConnect` (a connection).
#[unsafe(no_mangle)]
pub extern "system" fn AdsMgDisconnect(handle: AdsHandle) -> Unsigned32 {
    let _call = Call::begin("AdsMgDisconnect");
    crate::AdsDisconnect(handle)
}

/// `AdsMgResetCommStats`: starts the server's communication statistics
/// anew; tflip counts none (see `AdsMgGetCommStats`), so nothing changes.
#[unsafe(no_mangle)]
pub extern "system" fn AdsMgResetCommStats(handle: AdsHandle) -> Unsigned32 {
    let _call = Call::begin("AdsMgResetCommStats");
    crate::connection::checked(handle)
        .err()
        .unwrap_or(AE_SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AE_INVALID_CONNECTION_HANDLE, AE_INVALID_POINTER, AdsDisconnect, fixtures};
    use std::ffi::CStr;

    /// As the original engine answers.
    /// As the original engine's local server answers (probed).
    #[test]
    fn clones_cached_statements_and_server_calls() {
        let (connect, table) = crate::fixtures::open("all_types", c"all_types.adt");
        assert_eq!(crate::AdsGotoRecord(table, 2), AE_SUCCESS);
        let mut clone = 0;
        assert_eq!(
            unsafe { crate::AdsCloneTable(table, &mut clone) },
            AE_SUCCESS
        );
        let mut record = 0;
        unsafe { crate::AdsGetRecordNum(clone, crate::ADS_IGNOREFILTERS, &mut record) };
        assert_eq!(record, 2);
        assert_eq!(
            unsafe { crate::AdsCloneTable(connect, &mut clone) },
            crate::AE_INVALID_TABLE_HANDLE
        );
        let mut statement = 0;
        let sql = c"SELECT nosuch FROM all_types".as_ptr().cast_mut().cast();
        let rc = unsafe { crate::AdsCachePrepareSQL(connect, sql, &mut statement) };
        assert_eq!(rc, AE_SUCCESS);
        assert_ne!(statement, 0);
        let path = std::ffi::CString::new(format!(
            "{}/all_types.adt",
            crate::fixtures::dir("all_types").display()
        ))
        .unwrap();
        let path = path.as_ptr().cast_mut().cast();
        let (mut locks, mut len, mut size) = ([0u8; 64], 4, 16);
        let rc = unsafe {
            AdsMgGetLocks(
                connect,
                path,
                std::ptr::null_mut(),
                0,
                locks.as_mut_ptr(),
                &mut len,
                &mut size,
            )
        };
        assert_eq!((rc, len, size), (AE_SUCCESS, 0, 4));
        let (mut info, mut size, mut kind) = ([0u8; 300], 300, 0);
        let rc =
            unsafe { AdsMgGetLockOwner(connect, path, 1, info.as_mut_ptr(), &mut size, &mut kind) };
        assert_eq!((rc, size, kind), (AE_SUCCESS, 288, 1));
        let (mut threads, mut len, mut size) = ([0u8; 220], 2, 110);
        let rc = unsafe {
            AdsMgGetWorkerThreadActivity(connect, threads.as_mut_ptr(), &mut len, &mut size)
        };
        assert_eq!((rc, len, size), (AE_SUCCESS, 0, 110));
        assert_eq!(AdsMgDumpInternalTables(connect), AE_SUCCESS);
        assert_eq!(
            unsafe { AdsMgKillUser(connect, c"nobody".as_ptr().cast_mut().cast(), 0) },
            AE_SUCCESS
        );
        assert_eq!(crate::AdsDisableLocalConnections(), AE_SUCCESS);
        // The code page, the user-defined functions and the clipboard.
        let mut page = 0;
        let rc = unsafe { crate::AdsGetIntProperty(table, 1, &mut page) };
        assert_eq!((rc, page), (AE_SUCCESS, 1252));
        let rc = unsafe { crate::AdsGetIntProperty(connect, 2, &mut page) };
        assert_eq!(rc, crate::AE_INVALID_VALUE);
        let rc = unsafe { crate::AdsGetIntProperty(connect, 1, &mut page) };
        assert_eq!(rc, crate::AE_INVALID_HANDLE);
        unsafe extern "system" fn udf() -> Unsigned32 {
            0
        }
        assert_eq!(crate::AdsRegisterUDF(table, 1, Some(udf)), AE_SUCCESS);
        assert_eq!(
            crate::AdsRegisterUDF(table, 2, Some(udf)),
            crate::AE_INVALID_OPTION
        );
        assert_eq!(
            crate::AdsRegisterUDF(table, 1, None),
            crate::AE_INVALID_POINTER
        );
        assert_eq!(
            crate::AdsRegisterUDF(connect, 1, Some(udf)),
            crate::AE_INVALID_HANDLE
        );
        assert_eq!(crate::AdsRegisterUDF(0, 1, Some(udf)), AE_SUCCESS);
        let clip = |name: &CStr| unsafe {
            crate::AdsImageToClipboard(table, name.as_ptr().cast_mut().cast())
        };
        assert_eq!(clip(c"nosuch"), crate::AE_INVALID_FIELDNAME);
        assert_eq!(clip(c"c_char"), crate::AE_INVALID_FIELD_TYPE);
        assert_eq!(clip(c"c_memo"), crate::AE_INVALID_FIELD_TYPE);
        assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
    }

    #[test]
    fn server_types() {
        let connect = fixtures::connect("all_types");
        let mut value = 0xEEEE;
        assert_eq!(
            unsafe { AdsMgGetServerType(connect, &mut value) },
            AE_SUCCESS
        );
        let expected = match fixtures::is_remote() {
            true => server_type(&Platform::this()),
            false => ADS_MGMT_LOCAL_SERVER,
        };
        assert_eq!(value, expected);
        let rc = unsafe { AdsMgGetServerType(connect, std::ptr::null_mut()) };
        assert_eq!(rc, AE_INVALID_POINTER);
        let mut value = 0xEEEE;
        for handle in [0, 0x12345] {
            let rc = unsafe { AdsMgGetServerType(handle, &mut value) };
            assert_eq!((rc, value), (AE_INVALID_CONNECTION_HANDLE, 0xEEEE));
        }
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
    }

    #[test]
    fn platforms() {
        let platform = |os: &str, bits| Platform {
            os: os.to_owned(),
            bits,
        };
        assert_eq!(
            server_type(&platform("linux", 64)),
            ADS_MGMT_LINUX_SERVER_64_BIT
        );
        assert_eq!(server_type(&platform("linux", 32)), ADS_MGMT_LINUX_SERVER);
        assert_eq!(
            server_type(&platform("windows", 64)),
            ADS_MGMT_NT_SERVER_64_BIT
        );
        assert_eq!(server_type(&platform("windows", 32)), ADS_MGMT_NT_SERVER);
    }

    /// As the original engine fills it in, for the counts it keeps.
    #[test]
    fn activity_info() {
        let (connect, _table) = fixtures::open("indexes", c"indexed.adt");
        let info = |size: u16| {
            let mut buf = [0xEEu8; 200];
            let mut out = size;
            let rc = unsafe { AdsMgGetActivityInfo(connect, buf.as_mut_ptr(), &mut out) };
            (rc, out, buf)
        };
        let (rc, size, buf) = info(200);
        assert_eq!((rc, size), (AE_SUCCESS, 172));
        assert!(buf[172..].iter().all(|&b| b == 0xEE), "172 bytes written");
        let usage = |i: usize| {
            let at = 16 + 12 * i;
            u32::from_le_bytes(buf[at..at + 4].try_into().unwrap())
        };
        assert!(usage(1) >= 1, "connections");
        assert!(
            usage(2) >= 1 && usage(3) >= 1 && usage(4) >= 1,
            "work areas, tables, indexes"
        );
        let (rc, size, buf) = info(8);
        assert_eq!((rc, size), (AE_SUCCESS, 172));
        assert!(buf[8..].iter().all(|&b| b == 0xEE), "only what fits");
        let (rc, size, _) = info(0);
        assert_eq!((rc, size), (AE_INSUFFICIENT_BUFFER, 0));
        let mut size = 172;
        let rc = unsafe { AdsMgGetActivityInfo(connect, std::ptr::null_mut(), &mut size) };
        assert_eq!((rc, size), (AE_INVALID_POINTER, 172));
        let mut buf = [0u8; 172];
        let rc = unsafe { AdsMgGetActivityInfo(0x12345, buf.as_mut_ptr(), &mut size) };
        assert_eq!(rc, AE_INVALID_CONNECTION_HANDLE);
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
    }

    /// As the original engine answers.
    #[test]
    fn comm_stats() {
        let connect = fixtures::connect("all_types");
        let stats = |size: u16| {
            let mut buf = [0xEEu8; 64];
            let mut out = size;
            let rc = unsafe { AdsMgGetCommStats(connect, buf.as_mut_ptr(), &mut out) };
            (rc, out, buf)
        };
        let (rc, size, buf) = stats(64);
        assert_eq!((rc, size), (AE_SUCCESS, 48));
        assert!(buf[..48].iter().all(|&b| b == 0) && buf[48..].iter().all(|&b| b == 0xEE));
        let (rc, size, buf) = stats(4);
        assert_eq!((rc, size), (AE_SUCCESS, 48));
        assert_eq!(buf[3..5], [0, 0xEE], "only what fits");
        assert_eq!(stats(0).0, AE_INSUFFICIENT_BUFFER);
        let mut size = 48;
        let rc = unsafe { AdsMgGetCommStats(connect, std::ptr::null_mut(), &mut size) };
        assert_eq!((rc, size), (AE_INVALID_POINTER, 48));
        let mut buf = [0u8; 48];
        let rc = unsafe { AdsMgGetCommStats(0, buf.as_mut_ptr(), &mut size) };
        assert_eq!(rc, AE_INVALID_CONNECTION_HANDLE);
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
    }

    /// As the original engine answers.
    #[test]
    fn user_names() {
        let (connect, _table) = fixtures::open("indexes", c"indexed.adt");
        let users = |file: Option<&str>, len: u16, size: u16| {
            let mut buf = vec![0xEEu8; 288 * 4];
            let file = file.map(|f| std::ffi::CString::new(f).unwrap());
            let file = file
                .as_ref()
                .map_or(std::ptr::null_mut(), |f| f.as_ptr().cast_mut().cast());
            let (mut len, mut size) = (len, size);
            let rc =
                unsafe { AdsMgGetUserNames(connect, file, buf.as_mut_ptr(), &mut len, &mut size) };
            (rc, len, size, buf)
        };
        let (rc, len, size, buf) = users(None, 4, 288);
        assert_eq!((rc, size), (AE_SUCCESS, 288));
        assert!(len >= 1, "this connection at least");
        let name = |at: usize| buf[at..].split(|&b| b == 0).next().unwrap().to_vec();
        assert_eq!(name(0), sys::user_name().into_bytes());
        let program = std::env::current_exe().unwrap();
        let program = program.file_name().unwrap().to_string_lossy();
        assert_eq!(name(212), program.as_bytes()[..program.len().min(69)]);
        // A table this connection has open, by its full path.
        let full = format!("{}\\indexed.adt", fixtures::source("indexes"));
        let (rc, len, _, _) = users(Some(&full), 4, 288);
        assert!(rc == AE_SUCCESS && len >= 1, "{rc} {len}");
        let nothing = format!("{}\\nothing.adt", fixtures::source("indexes"));
        let (rc, len, _, _) = users(Some(&nothing), 4, 288);
        assert_eq!((rc, len), (crate::FILE_NOT_OPEN, 0));
        assert_eq!(users(Some(""), 4, 288).0, crate::AE_INVALID_PATH);
        // Entries that do not fit are left out; 0 entries or bytes fail.
        let (rc, len, _, _) = users(None, 1, 288);
        assert_eq!((rc, len), (AE_SUCCESS, 1));
        assert_eq!(users(None, 0, 288).0, AE_INSUFFICIENT_BUFFER);
        assert_eq!(users(None, 4, 0).0, AE_INSUFFICIENT_BUFFER);
        // A smaller size cuts each entry.
        let (rc, _, size, buf) = users(None, 1, 10);
        assert_eq!((rc, size, buf[10]), (AE_SUCCESS, 288, 0xEE));
        // A larger one pads it with zeros.
        let (rc, _, _, buf) = users(None, 1, 300);
        assert_eq!((rc, buf[299], buf[300]), (AE_SUCCESS, 0, 0xEE));
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
    }

    /// As the original engine answers.
    #[test]
    fn open_tables() {
        // A copy of its own, as other tests open the fixture's table too.
        let source = fixtures::scratch("open_tables", "indexes");
        let connect = fixtures::connect_to(&source);
        let name = c"indexed.adt".as_ptr().cast_mut().cast();
        let null = std::ptr::null_mut();
        for _ in 0..2 {
            let mut table = 0;
            let rc = unsafe {
                crate::AdsOpenTable(connect, name, null, crate::ADS_ADT, 1, 1, 1, 0, &mut table)
            };
            assert_eq!(rc, AE_SUCCESS);
        }
        let ours = |name: &str| name.contains("open_tables") && name.ends_with("indexed.adt");
        let tables = |user: Option<&str>, number: u16, len: u16, size: u16| {
            let mut buf = vec![0xEEu8; 262 * 16];
            let user = user.map(|u| std::ffi::CString::new(u).unwrap());
            let user = user
                .as_ref()
                .map_or(std::ptr::null_mut(), |u| u.as_ptr().cast_mut().cast());
            let (mut len, mut size) = (len, size);
            let rc = unsafe {
                AdsMgGetOpenTables(connect, user, number, buf.as_mut_ptr(), &mut len, &mut size)
            };
            (rc, len, size, buf)
        };
        let entry = |buf: &[u8], i: usize| {
            let at = 262 * i;
            let name = buf[at..at + 260]
                .split(|&b| b == 0)
                .next()
                .unwrap()
                .to_vec();
            (
                String::from_utf8(name).unwrap(),
                u16::from_le_bytes([buf[at + 260], buf[at + 261]]),
            )
        };
        // Each file once; other tests' tables may be open too.
        let (rc, len, size, buf) = tables(None, 0, 16, 262);
        assert_eq!((rc, size), (AE_SUCCESS, 262));
        let names: Vec<(String, u16)> = (0..usize::from(len)).map(|i| entry(&buf, i)).collect();
        let indexed = names.iter().filter(|(name, _)| ours(name)).count();
        assert_eq!(indexed, 1, "{names:?}");
        let found = names.iter().find(|(name, _)| ours(name));
        assert_eq!(
            found.map(|(_, locking)| *locking),
            Some(ADS_MGMT_ADT_LOCKING)
        );
        // A user's tables: every handle.
        let (rc, len, _, buf) = tables(Some(&sys::user_name()), 0, 16, 262);
        assert_eq!(rc, AE_SUCCESS);
        let indexed = (0..usize::from(len))
            .filter(|&i| ours(&entry(&buf, i).0))
            .count();
        assert!(indexed >= 2);
        let (rc, len, _, _) = tables(Some("nobody here"), 0, 16, 262);
        assert_eq!((rc, len), (crate::USER_NOT_FOUND, 0));
        assert_eq!(tables(Some(""), 0, 16, 262).0, crate::AE_INVALID_USERNAME);
        assert_eq!(tables(None, 1, 16, 262).0, crate::USER_NOT_FOUND);
        assert_eq!(tables(None, 0, 0, 262).0, AE_INSUFFICIENT_BUFFER);
        assert_eq!(tables(None, 0, 16, 0).0, AE_INSUFFICIENT_BUFFER);
        let (rc, len, _, _) = tables(None, 0, 1, 262);
        assert_eq!((rc, len), (AE_SUCCESS, 1));
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
    }

    /// As the original engine answers.
    #[test]
    fn config_info() {
        let connect = fixtures::connect("all_types");
        let config = |params_size: u16, memory_size: u16| {
            let (mut params, mut memory) = ([0xEEu8; 1000], [0xEEu8; 100]);
            let (mut ps, mut ms) = (params_size, memory_size);
            let rc = unsafe {
                AdsMgGetConfigInfo(
                    connect,
                    params.as_mut_ptr(),
                    &mut ps,
                    memory.as_mut_ptr(),
                    &mut ms,
                )
            };
            (rc, ps, ms, params, memory)
        };
        let (rc, ps, ms, params, memory) = config(864, 56);
        assert_eq!((rc, ps, ms), (AE_SUCCESS, 864, 56));
        let u32_at = |at: usize| u32::from_le_bytes(params[at..at + 4].try_into().unwrap());
        assert_eq!(
            [u32_at(0), u32_at(4), u32_at(8), u32_at(12), u32_at(16)],
            [20, 100, 50, 75, 500]
        );
        assert_eq!(&params[0x38..0x3c], b"C:\\\0");
        assert_eq!(
            (u32_at(0x344), params[0x34e], u32_at(0x35a)),
            (20000, 1, 100)
        );
        assert!(memory[..56].iter().all(|&b| b == 0) && params[864] == 0xEE);
        // Smaller sizes get what fits and keep their value; larger ones
        // are filled with zeros and set to the structure's size.
        let (rc, ps, ms, params, memory) = config(100, 20);
        assert_eq!(
            (rc, ps, ms, params[100], memory[20]),
            (AE_SUCCESS, 100, 20, 0xEE, 0xEE)
        );
        let (rc, ps, ms, params, memory) = config(1000, 100);
        assert_eq!(
            (rc, ps, ms, params[999], memory[99]),
            (AE_SUCCESS, 864, 56, 0, 0)
        );
        assert_eq!(config(0, 56).0, AE_INSUFFICIENT_BUFFER);
        assert_eq!(config(864, 0).0, AE_INSUFFICIENT_BUFFER);
        let mut ps = 864;
        let null = std::ptr::null_mut();
        let rc = unsafe { AdsMgGetConfigInfo(connect, null, &mut ps, null, &mut ps) };
        assert_eq!(rc, AE_INVALID_POINTER);
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
    }

    /// As the original engine answers.
    #[test]
    fn install_info() {
        let connect = fixtures::connect("all_types");
        let info = |size: u16| {
            let mut buf = [0xEEu8; 200];
            let mut out = size;
            let rc = unsafe { AdsMgGetInstallInfo(connect, buf.as_mut_ptr(), &mut out) };
            (rc, out, buf)
        };
        let (rc, size, buf) = info(200);
        assert_eq!((rc, size, buf[144]), (AE_SUCCESS, 144, 0xEE));
        assert_eq!(buf[..4], 5u32.to_le_bytes());
        assert_eq!(&buf[40..53], b"12.0.0.0    \0");
        assert_eq!(&buf[88..99], b"ENGL(AMER)\0");
        let (rc, size, buf) = info(10);
        assert_eq!((rc, size, buf[10]), (AE_SUCCESS, 144, 0xEE));
        assert_eq!(info(0).0, AE_INSUFFICIENT_BUFFER);
        let mut buf = [0u8; 144];
        let mut size = 144;
        assert_eq!(
            unsafe { AdsMgGetInstallInfo(0, buf.as_mut_ptr(), &mut size) },
            AE_INVALID_CONNECTION_HANDLE
        );
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
    }

    /// As the original engine answers.
    #[test]
    fn open_indexes() {
        // A copy of its own, as other tests open the fixture's table too.
        let source = fixtures::scratch("open_indexes", "indexes");
        let connect = fixtures::connect_to(&source);
        let name = c"indexed.adt".as_ptr().cast_mut().cast();
        let null = std::ptr::null_mut();
        for _ in 0..2 {
            let mut table = 0;
            let rc = unsafe {
                crate::AdsOpenTable(connect, name, null, crate::ADS_ADT, 1, 1, 1, 0, &mut table)
            };
            assert_eq!(rc, AE_SUCCESS);
        }
        let indexes = |table: Option<&str>, user: Option<&str>, len: u16| {
            let mut buf = vec![0xEEu8; 260 * 16];
            let cstring = |s: Option<&str>| s.map(|s| std::ffi::CString::new(s).unwrap());
            let (table, user) = (cstring(table), cstring(user));
            let ptr = |s: &Option<std::ffi::CString>| {
                s.as_ref()
                    .map_or(std::ptr::null_mut(), |s| s.as_ptr().cast_mut().cast())
            };
            let (mut len, mut size) = (len, 260);
            let rc = unsafe {
                AdsMgGetOpenIndexes(
                    connect,
                    ptr(&table),
                    ptr(&user),
                    0,
                    buf.as_mut_ptr(),
                    &mut len,
                    &mut size,
                )
            };
            let names: Vec<String> = (0..usize::from(len))
                .map(|i| {
                    let entry = &buf[260 * i..260 * (i + 1)];
                    String::from_utf8_lossy(entry.split(|&b| b == 0).next().unwrap()).into_owned()
                })
                .collect();
            (rc, names)
        };
        let ours = |names: &[String]| {
            let ours = |n: &&String| n.contains("open_indexes") && n.ends_with("indexed.adi");
            names.iter().filter(ours).count()
        };
        // Each file once; other tests' indexes may be open too.
        let (rc, names) = indexes(None, None, 16);
        assert_eq!((rc, ours(&names)), (AE_SUCCESS, 1), "{names:?}");
        // A table's: once per handle.
        let full = format!("{source}\\indexed.adt");
        let (rc, names) = indexes(Some(&full), None, 16);
        assert_eq!((rc, names.len(), ours(&names)), (AE_SUCCESS, 2, 2));
        let (rc, names) = indexes(None, Some(&sys::user_name()), 16);
        assert!(rc == AE_SUCCESS && ours(&names) >= 2);
        let nothing = format!("{source}\\nothing.adt");
        assert_eq!(indexes(Some(&nothing), None, 16).0, crate::FILE_NOT_OPEN);
        assert_eq!(indexes(Some(""), None, 16).0, crate::AE_INVALID_PATH);
        assert_eq!(
            indexes(None, Some("nobody here"), 16).0,
            crate::USER_NOT_FOUND
        );
        assert_eq!(indexes(None, None, 0).0, AE_INSUFFICIENT_BUFFER);
        assert_eq!(AdsDisconnect(connect), AE_SUCCESS);
    }

    crate::fixtures::remote_twins!(
        server_types,
        activity_info,
        comm_stats,
        user_names,
        open_tables,
        config_info,
        install_info,
        open_indexes
    );
}
