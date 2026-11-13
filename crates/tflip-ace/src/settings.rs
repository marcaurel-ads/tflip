//! Engine information and the settings of the process and its
//! connections: date formats, the epoch, the exact setting, collations, the
//! search and default paths, and the callbacks clients register.

use tflip_engine::Error;
use tflip_protocol::Api;

use crate::backend::{self, api};
use crate::ffi::text;
use crate::handle::connection_check;
use crate::last_error::{Call, fail, fail_handle};
use crate::log::{Hex, stub};
use crate::{
    ADS_FALSE, ADS_TRUE, AE_INVALID_CONNECTION_HANDLE, AE_INVALID_POINTER, AE_SUCCESS, AdsHandle,
    LOCAL, Unsigned8, Unsigned16, Unsigned32, ansi, code, error_code, write_cut, write_cut_at,
};

const VERSION_MAJOR: Unsigned32 = 12;
const VERSION_MINOR: Unsigned32 = 0;
const VERSION_DESC: &[u8] = b"tflip ace32";

/// `AdsGetVersion`: the engine version and a description string.
///
/// # Safety
///
/// Every non-null pointer must be valid for writes; `desc` must hold
/// `*desc_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetVersion(
    major: *mut Unsigned32,
    minor: *mut Unsigned32,
    letter: *mut Unsigned8,
    desc: *mut Unsigned8,
    desc_len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetVersion");
    // As in the original engine, the first four may not be NULL
    // (`fixtures/errors/param_n.json`); the length of the description may.
    for (number, null) in [
        major.is_null(),
        minor.is_null(),
        letter.is_null(),
        desc.is_null(),
    ]
    .into_iter()
    .enumerate()
    {
        if null {
            return crate::last_error::null_param(number as u32 + 1);
        }
    }
    // One at a time: a client may pass the same variable for several.
    // SAFETY: `major` is null or writable, as # Safety says.
    if let Some(major) = unsafe { major.as_mut() } {
        *major = VERSION_MAJOR;
    }
    // SAFETY: `minor` is null or writable, as # Safety says.
    if let Some(minor) = unsafe { minor.as_mut() } {
        *minor = VERSION_MINOR;
    }
    // SAFETY: `letter` is null or writable, as # Safety says.
    if let Some(letter) = unsafe { letter.as_mut() } {
        *letter = b' ';
    }
    if desc_len.is_null() {
        return AE_SUCCESS;
    }
    // SAFETY: not null, so readable, as # Safety says.
    let room = unsafe { *desc_len };
    if room > 0 {
        let n = VERSION_DESC.len().min(usize::from(room) - 1);
        // SAFETY: `desc` holds `*desc_len` bytes, which the copy and its NUL
        // stay inside, and `desc_len` is writable, as # Safety says.
        unsafe {
            std::ptr::copy_nonoverlapping(VERSION_DESC.as_ptr(), desc, n);
            *desc.add(n) = 0;
            *desc_len = n as Unsigned16;
        }
    }
    AE_SUCCESS
}

/// What `AdsCacheOpenTables` and `AdsCacheOpenCursors` last set, if
/// anything: the sizes of the caches of every connection to a server (the
/// local server's keeps its own, see [`set_cache_size`]).
static CACHE_SIZES: std::sync::Mutex<[Option<Unsigned16>; 2]> = std::sync::Mutex::new([None; 2]);

/// The sizes `AdsCacheOpenTables` (`[0]`) and `AdsCacheOpenCursors` (`[1]`)
/// set, for a new connection to a server to take.
pub(crate) fn cache_sizes() -> [Option<Unsigned16>; 2] {
    *CACHE_SIZES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Forgets what `AdsCacheOpenTables` and `AdsCacheOpenCursors` set, as
/// if they had never been called (for a test that called them), and turns
/// the local server's caches off.
#[cfg(test)]
pub(crate) fn forget_cache_sizes() {
    *CACHE_SIZES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = [None; 2];
    tflip_engine::settings::set_local_caching(&crate::LOCAL, false);
}

/// Sets how many closed tables (or, with `cursors`, SQL statements'
/// tables) every connection keeps open, now and for the connections made
/// later: those to a server on it, and the local server's if its caching
/// is on (`tflip.ini`'s `[cache] local`, see [`crate::config`]); else it
/// keeps none.
fn set_cache_size(cursors: bool, size: Unsigned16) {
    CACHE_SIZES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)[usize::from(cursors)] = Some(size);
    tflip_engine::settings::set_cache_sizes(&crate::LOCAL, cursors, size);
    for remote in backend::all_remotes() {
        let _ = remote
            .client
            .set_cache_size(remote.connection, cursors, size);
    }
}

/// `AdsCacheOpenCursors`: of how many closed SQL statements a connection
/// keeps the tables open for reuse (25 by default); any number is
/// accepted, as in the original engine.
///
/// A connection to `tflipd` keeps them open on the server (see
/// `tflip_engine`'s cache); the local server keeps them only if
/// `tflip.ini`'s `[cache] local` says so (see [`crate::config`]), else
/// there it is accepted and ignored (see `docs/DIFFERENCES.md`).
#[unsafe(no_mangle)]
pub extern "system" fn AdsCacheOpenCursors(open: Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCacheOpenCursors");
    set_cache_size(true, open);
    AE_SUCCESS
}

/// `AdsCacheOpenTables`: how many closed tables a connection keeps open
/// for reuse (none by default; `AdsCloseCachedTables` closes them). Any
/// number is accepted, as in the original engine.
///
/// A connection to `tflipd` keeps them open on the server (see
/// `tflip_engine`'s cache); the local server keeps them only if
/// `tflip.ini`'s `[cache] local` says so (see [`crate::config`]), else
/// there it is accepted and ignored (see `docs/DIFFERENCES.md`).
#[unsafe(no_mangle)]
pub extern "system" fn AdsCacheOpenTables(open: Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCacheOpenTables");
    set_cache_size(false, open);
    AE_SUCCESS
}

/// Sets the process-wide date format, in the local session and in the
/// session of every connection to a server.
fn set_date_format(format: String) -> Result<(), Error> {
    LOCAL.set_date_format(0, format.clone())?;
    for remote in backend::all_remotes() {
        let _ = remote.client.set_date_format(0, format.clone());
    }
    Ok(())
}

/// Sets the process-wide "exact" setting, in the local session and in the
/// session of every connection to a server.
fn set_exact(exact: bool) {
    let _ = LOCAL.set_exact(0, exact);
    for remote in backend::all_remotes() {
        let _ = remote.client.set_exact(0, exact);
    }
}

/// `AdsSetDateFormat`: the date format for converting dates to and from text,
/// such as ACE's default `MM/DD/CCYY` or `DD.MM.YYYY`. A format the original
/// engine refuses fails with `AE_INVALID_DATEFORMAT` (Windows-style
/// `M/d/yyyy`, which ARC passes, among them) or, longer than 11 characters,
/// with `AE_DATA_TOO_LONG`, and leaves the format as it was. See
/// [`tflip_engine::settings::set_date_format`] for the rules.
///
/// # Safety
///
/// `format` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetDateFormat(format: *mut Unsigned8) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetDateFormat");
    // SAFETY: `format` is null or NUL-terminated, as # Safety says.
    let Some(format) = (unsafe { text(format) }) else {
        return crate::last_error::null_param(1);
    };
    let detail = format!("format {format:?}");
    match set_date_format(format) {
        Ok(()) => AE_SUCCESS,
        Err(error) => {
            crate::last_error::detail(detail);
            error_code(error)
        }
    }
}

/// `AdsGetDateFormat`: the date format set by `AdsSetDateFormat`, as the
/// original engine keeps it (upper case, `YYYY` as `CCYY`: `dd.mm.yyyy` reads
/// back as `DD.MM.CCYY`), in `format` of `*len` bytes (NUL-terminated);
/// `*len` is set to its length. As in the original engine, a buffer that is
/// too small fails with `AE_INSUFFICIENT_BUFFER` and `*len` set to the length
/// needed including the NUL, after the bytes that fit are copied.
///
/// # Safety
///
/// `format` must be null or hold `*len` bytes; `len` must be null or valid
/// for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetDateFormat(
    format: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    // As in the original engine, the last error stays as it was, unless
    // there was none.
    let current = ansi::from_str_lossy(&tflip_engine::settings::date_format(&LOCAL));
    crate::last_error::keeping_pending("AdsGetDateFormat", || {
        // SAFETY: `format` and `len` are as # Safety says.
        unsafe { write_cut(&current, format, len) }
    })
}

/// `AdsSetDateFormat60`: sets the date format of one connection, checked as
/// `AdsSetDateFormat` checks it; its tables convert dates with it. Connection
/// 0 sets the process-wide format, as `AdsSetDateFormat` does, which every
/// connection uses from then on, its own format forgotten (as in the
/// original engine). A handle that is not a connection fails with
/// `AE_INVALID_HANDLE`.
///
/// # Safety
///
/// `format` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetDateFormat60(
    connect: AdsHandle,
    format: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetDateFormat60");
    // SAFETY: `format` is null or NUL-terminated, as # Safety says.
    let Some(format) = (unsafe { text(format) }) else {
        return crate::last_error::null_param(2);
    };
    let detail = format!("format {format:?}");
    let result = match connect {
        0 => set_date_format(format),
        _ => api(connect).set_date_format(connect, format),
    };
    match result {
        Ok(()) => AE_SUCCESS,
        Err(error) => {
            crate::last_error::detail(detail);
            error_code(error)
        }
    }
}

/// `AdsGetDateFormat60`: the date format of one connection, its own or the
/// process-wide one, as `AdsGetDateFormat` returns it; connection 0 is the
/// process-wide one. A handle that is not a connection fails with
/// `AE_INVALID_HANDLE`.
///
/// # Safety
///
/// `format` must be null or hold `*len` bytes; `len` must be null or valid
/// for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetDateFormat60(
    connect: AdsHandle,
    format: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetDateFormat60");
    match api(connect).date_format(connect) {
        Ok(current) => {
            let current = ansi::from_str_lossy(&current);
            // SAFETY: `format` and `len` are as # Safety says.
            unsafe { write_cut(&current, format, len) }
        }
        Err(error) => error_code(error),
    }
}

/// `AdsSetExact`: the process-wide "exact" setting, whether string
/// comparisons with `=`, `<`, `>`, `<=` and `>=` in filters ignore trailing
/// spaces (any non-zero value is `ADS_TRUE`). As in the original engine, it
/// sets every open connection's and table's as well. Nothing evaluates
/// filters yet, so the setting is only kept.
#[unsafe(no_mangle)]
pub extern "system" fn AdsSetExact(exact: Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetExact");
    set_exact(exact != ADS_FALSE);
    AE_SUCCESS
}

/// `AdsGetExact`: the process-wide "exact" setting, which new connections
/// start with; `ADS_FALSE` until set.
///
/// # Safety
///
/// `exact` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetExact(exact: *mut Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetExact");
    if exact.is_null() {
        return crate::last_error::null_param(1);
    }
    let value = tflip_engine::settings::exact(&LOCAL);
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *exact = if value { ADS_TRUE } else { ADS_FALSE } };
    AE_SUCCESS
}

/// `AdsSetExact22`: the "exact" setting of a connection or table (see
/// `AdsSetExact`); handle 0 is the process-wide one. As in the original
/// engine, a connection's setting is only what tables opened on it from
/// then on start with (the help says otherwise), and any other handle fails
/// with `AE_INVALID_HANDLE`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsSetExact22(obj: AdsHandle, exact: Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetExact22");
    let result = match obj {
        0 => {
            set_exact(exact != ADS_FALSE);
            Ok(())
        }
        _ => api(obj).set_exact(obj, exact != ADS_FALSE),
    };
    code(result)
}

/// `AdsGetExact22`: the "exact" setting of a connection or table; handle 0
/// is the process-wide one. As in the original engine, any other handle
/// fails with `AE_INVALID_CONNECTION_HANDLE` (where `AdsSetExact22` reports
/// `AE_INVALID_HANDLE`).
///
/// # Safety
///
/// `exact` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetExact22(obj: AdsHandle, exact: *mut Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetExact22");
    let value = match api(obj).exact(obj) {
        Ok(value) => value,
        Err(_) => return fail_handle(AE_INVALID_CONNECTION_HANDLE),
    };
    if exact.is_null() {
        return crate::last_error::null_param(2);
    }
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *exact = if value { ADS_TRUE } else { ADS_FALSE } };
    AE_SUCCESS
}

/// `AdsGetDecimals`: the number of decimal places for converting numbers to
/// text, as set by `AdsSetDecimals`; 2 by default, as in the original engine.
///
/// # Safety
///
/// `decimals` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetDecimals(decimals: *mut Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetDecimals");
    if decimals.is_null() {
        return crate::last_error::null_param(1);
    }
    let value = tflip_engine::settings::decimals(&LOCAL);
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *decimals = value };
    AE_SUCCESS
}

/// `AdsSetDecimals`: sets the number of decimal places for converting
/// numbers to text. Nothing converts with it yet; Double fields use their
/// own decimals (as in the original engine).
#[unsafe(no_mangle)]
pub extern "system" fn AdsSetDecimals(decimals: Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetDecimals");
    tflip_engine::settings::set_decimals(&LOCAL, decimals);
    AE_SUCCESS
}

/// `AdsGetDeleted`: whether deleted records are visible.
///
/// # Safety
///
/// `use_deleted` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetDeleted(use_deleted: *mut Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetDeleted");
    if use_deleted.is_null() {
        return crate::last_error::null_param(1);
    }
    let shown = tflip_engine::settings::show_deleted(&LOCAL);
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *use_deleted = if shown { ADS_TRUE } else { ADS_FALSE } };
    AE_SUCCESS
}

/// `AdsShowDeleted`: makes the deleted records of DBF tables visible
/// (`ADS_TRUE`) or hidden, on every server connected and connected later.
#[unsafe(no_mangle)]
pub extern "system" fn AdsShowDeleted(show_deleted: Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsShowDeleted");
    let show = show_deleted != ADS_FALSE;
    tflip_engine::settings::set_show_deleted(&LOCAL, show);
    for remote in crate::backend::all_remotes() {
        let _ = remote.client.set_show_deleted(show);
    }
    AE_SUCCESS
}

/// `AdsSetServerType`: which kinds of server `AdsConnect` (and
/// `AdsConnect26` and `AdsConnect60` without server types of their own) may
/// use, as `ADS_LOCAL_SERVER | ADS_REMOTE_SERVER | ADS_AIS_SERVER` flags.
/// Until set, remote and Internet servers, as in the original engine.
/// `AdsConnect101` takes the server type from its connect string instead.
#[unsafe(no_mangle)]
pub extern "system" fn AdsSetServerType(server_options: Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetServerType");
    tflip_engine::settings::set_server_types(&LOCAL, server_options);
    AE_SUCCESS
}

/// `AdsSetProperty`: sets property `operation` of the object `obj` to
/// `value`.
///
/// Not declared in the SDK's `ace.h`; `aceunpub.pas` declares it with a
/// 32-bit value, in which property 87 passes a pointer, so this takes one
/// the size of a pointer. The properties ARC sets were probed
/// (`tools/ace/sql/props.txt` and `props39.txt`):
///
/// - 27, not to use the cached error codes of remote connections: tflip
///   caches none, so it is accepted on any handle.
/// - 39, on a statement: the next `AdsVerifySQL` checks the syntax alone,
///   whatever the value. Any other handle fails with
///   `AE_INVALID_SQLSTATEMENT_HANDLE`.
/// - 87, on any handle: writes 1 to the `UNSIGNED32` `value` points to, as
///   the original engine does with its AES libraries (without them it
///   writes 0); tflip creates AES tables. A null pointer is ignored.
///
/// Any other property is accepted and ignored.
///
/// # Safety
///
/// For property 87, `value` must be null or valid for writes of an
/// `UNSIGNED32`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetProperty(
    obj: AdsHandle,
    operation: Unsigned32,
    value: usize,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetProperty");
    match operation {
        27 => AE_SUCCESS,
        39 => {
            // A value that is no handle is reported as one, but the call
            // still answers with the statement code.
            if api(obj).handle_kind(obj) == Ok(None) {
                crate::last_error::fail_detail(
                    crate::AE_INVALID_HANDLE,
                    crate::last_error::NOT_A_HANDLE,
                );
                return crate::AE_INVALID_SQLSTATEMENT_HANDLE;
            }
            crate::statement::set_statement(
                obj,
                Some(tflip_engine::statement::Setting::ParseOnly),
                AE_SUCCESS,
            )
        }
        87 => {
            // SAFETY: `value` is null or writable, as # Safety says.
            if let Some(available) = unsafe { (value as *mut Unsigned32).as_mut() } {
                *available = 1;
            }
            AE_SUCCESS
        }
        _ => {
            stub!("AdsSetProperty", Hex(obj), operation, value);
            AE_SUCCESS
        }
    }
}

/// A progress callback: called with the percentage done and the ID given at
/// registration.
pub type ProgressCallback =
    unsafe extern "system" fn(percent: Unsigned16, callback_id: Unsigned32) -> Unsigned32;

/// `AdsRegisterCallbackFunction`: registers a callback for progress reports
/// during long operations.
///
/// Nothing reports progress yet, so the callback is never called.
#[unsafe(no_mangle)]
pub extern "system" fn AdsRegisterCallbackFunction(
    callback: Option<ProgressCallback>,
    callback_id: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsRegisterCallbackFunction");
    stub!("AdsRegisterCallbackFunction", callback, callback_id);
    AE_SUCCESS
}

/// A progress callback with a 64-bit ID, for
/// [`AdsRegisterCallbackFunction101`].
pub type ProgressCallback101 =
    unsafe extern "system" fn(percent: Unsigned16, callback_id: i64) -> Unsigned32;

/// `AdsRegisterCallbackFunction101`: [`AdsRegisterCallbackFunction`] with a
/// 64-bit ID. A null callback fails with `AE_INVALID_POINTER`, as in the
/// original engine.
///
/// Nothing reports progress yet, so the callback is never called.
#[unsafe(no_mangle)]
pub extern "system" fn AdsRegisterCallbackFunction101(
    callback: Option<ProgressCallback101>,
    callback_id: i64,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsRegisterCallbackFunction101");
    if callback.is_none() {
        return fail(AE_INVALID_POINTER);
    }
    stub!("AdsRegisterCallbackFunction101", callback, callback_id);
    AE_SUCCESS
}

/// `AdsClearCallbackFunction`: unregisters the progress callback.
///
/// No callback is kept, so there is nothing to clear.
#[unsafe(no_mangle)]
pub extern "system" fn AdsClearCallbackFunction() -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsClearCallbackFunction");
    AE_SUCCESS
}

/// `AdsGetEpoch`: the first year of the hundred that years of two digits
/// fall in (1900 until set).
///
/// # Safety
///
/// `century` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetEpoch(century: *mut Unsigned16) -> Unsigned32 {
    let _call = Call::begin("AdsGetEpoch");
    let epoch = tflip_engine::settings::epoch(&LOCAL);
    // SAFETY: `century` is null or writable, as # Safety says.
    unsafe { crate::handle::answer_at(Ok(()), century, epoch, 1) }
}

/// `AdsSetEpoch`: sets the epoch, process-wide (on every server this
/// process is connected to too); as in the original engine, any value is
/// taken. Under 1950, `49` is 2049 and `50` is 1950.
#[unsafe(no_mangle)]
pub extern "system" fn AdsSetEpoch(century: Unsigned16) -> Unsigned32 {
    let _call = Call::begin("AdsSetEpoch");
    tflip_engine::settings::set_epoch(&LOCAL, century);
    for remote in backend::all_remotes() {
        let _ = remote.client.set_epoch(century);
    }
    AE_SUCCESS
}

/// `AdsGetSearchPath`: the search path set by `AdsSetSearchPath` (empty
/// until set), in `buf` of `*len` bytes. A buffer that is too small gets
/// what fits and `AE_INSUFFICIENT_BUFFER`, with `*len` set to the length
/// needed with the NUL.
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetSearchPath(
    buf: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetSearchPath");
    let path = ansi::from_str_lossy(&tflip_engine::settings::search_path(&LOCAL));
    // SAFETY: `buf` and `len` are as # Safety says.
    unsafe { write_cut(&path, buf, len) }
}

/// `AdsSetSearchPath`: sets the search path, where the original engine also
/// looks for tables it does not find; tflip keeps it but does not look
/// there. Null clears it.
///
/// # Safety
///
/// `path` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetSearchPath(path: *mut Unsigned8) -> Unsigned32 {
    let _call = Call::begin("AdsSetSearchPath");
    // SAFETY: `path` is null or NUL-terminated, as # Safety says.
    let path = unsafe { text(path) }.unwrap_or_default();
    tflip_engine::settings::set_search_path(&LOCAL, &path);
    AE_SUCCESS
}

/// `AdsGetDefault`: the default path set by `AdsSetDefault` (empty until
/// set), in `buf` of `*len` bytes, as `AdsGetSearchPath` returns the search
/// path.
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetDefault(
    buf: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetDefault");
    let path = ansi::from_str_lossy(&tflip_engine::settings::default_path(&LOCAL));
    // SAFETY: `buf` and `len` are as # Safety says.
    unsafe { write_cut(&path, buf, len) }
}

/// `AdsSetDefault`: sets the default path, where the original engine opens
/// and creates tables given without a path and without a connection; tflip
/// keeps it but does not look there. As in the original engine, a path gets
/// a `\` at its end, and null or an empty path clears it.
///
/// # Safety
///
/// `path` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetDefault(path: *mut Unsigned8) -> Unsigned32 {
    let _call = Call::begin("AdsSetDefault");
    // SAFETY: `path` is null or NUL-terminated, as # Safety says.
    let path = unsafe { text(path) }.unwrap_or_default();
    tflip_engine::settings::set_default_path(&LOCAL, &path);
    AE_SUCCESS
}

/// `AdsClearDefault`: clears the default path.
#[unsafe(no_mangle)]
pub extern "system" fn AdsClearDefault() -> Unsigned32 {
    let _call = Call::begin("AdsClearDefault");
    tflip_engine::settings::set_default_path(&LOCAL, "");
    AE_SUCCESS
}

/// `AdsGetCollation`: the collation set on a connection with
/// `AdsSetCollation`, empty until set, as in the original engine.
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetCollation(
    connect: AdsHandle,
    buf: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetCollation");
    if let Err(code) = connection_check(connect) {
        return code;
    }
    match api(connect).collation(connect) {
        Ok(collation) => {
            let collation = ansi::from_str_lossy(&collation);
            // SAFETY: `buf` and `len` are as # Safety says.
            unsafe { write_cut_at(&collation, buf, len, 2) }
        }
        Err(error) => error_code(error),
    }
}

/// Checks a collation `name` the way the original engine does, for
/// `AdsSetCollation` and (with `statement`) `AdsStmtSetTableCollation`, on
/// every kind of connection alike (probed on a free-table, a local
/// dictionary and a remote dictionary connection, `tools/ace/collation`).
/// Returns the name to keep, or the error code, recorded with the name.
///
/// An empty name clears the collation. Else it must be in the repository
/// (`adscollate.adt`) regardless of case and of trailing blanks, and is kept
/// in the repository's spelling. A statement also takes `ANSI` and `OEM`
/// (regardless of case, no blanks), and `:` and an ICU locale
/// ([`tflip_storage::collation::Dynamic::is_icu_locale`]), refused with 1564
/// where the engine has no collator for it. Anything else is 7147. Where the
/// repository is not reachable, a name is kept as given, as tflip cannot tell
/// it is wrong.
pub(crate) fn check_collation(name: String, statement: bool) -> Result<String, Unsigned32> {
    if name.is_empty() {
        return Ok(name);
    }
    if statement {
        if let Some(locale) = name.strip_prefix(':') {
            return match tflip_storage::collation::Dynamic::is_icu_locale(locale) {
                true => Ok(name),
                false => Err(crate::last_error::fail_detail(
                    crate::AE_COLLATOR_NOT_OPENED,
                    if locale.is_empty() { "?" } else { locale },
                )),
            };
        }
        if name.eq_ignore_ascii_case("ANSI") || name.eq_ignore_ascii_case("OEM") {
            return Ok(name);
        }
    }
    let unknown = || {
        Err(crate::last_error::fail_detail(
            crate::AE_COLLATION_NAME_NOT_FOUND,
            &name,
        ))
    };
    let Some(dir) = crate::config::collation_dir() else {
        return Ok(name);
    };
    if !dir.join("adscollate.adt").exists() {
        return Ok(name);
    }
    if name.starts_with(char::is_whitespace) {
        return unknown();
    }
    match tflip_storage::collation::Dynamic::canonical(&dir, name.trim_end()) {
        Some(canonical) => Ok(canonical),
        None => unknown(),
    }
}

/// `AdsSetCollation`: sets a connection's dynamic collation for the tables
/// it opens later without one of their own. A name the repository
/// (`adscollate.adt`) does not have is refused with 7147, as the original
/// engine does; see [`check_collation`].
///
/// # Safety
///
/// `collation` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetCollation(
    connect: AdsHandle,
    collation: *mut Unsigned8,
) -> Unsigned32 {
    let _call = Call::begin("AdsSetCollation");
    if let Err(code) = connection_check(connect) {
        return code;
    }
    // SAFETY: `collation` is null or NUL-terminated, as # Safety says.
    let collation = unsafe { text(collation) }.unwrap_or_default();
    let collation = match check_collation(collation, false) {
        Ok(name) => name,
        Err(code) => return code,
    };
    code(api(connect).set_collation(connect, collation))
}

/// `AdsGetCollationLang`: `unknown`, as the original engine reports.
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetCollationLang(
    buf: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetCollationLang");
    // SAFETY: `buf` and `len` are as # Safety says.
    unsafe { write_cut(b"unknown", buf, len) }
}

/// `AdsSetCollationLang`: taken and ignored, as by the original engine,
/// which reports `unknown` after it.
///
/// # Safety
///
/// `lang` is not read.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetCollationLang(lang: *mut Unsigned8) -> Unsigned32 {
    let _call = Call::begin("AdsSetCollationLang");
    let _ = lang;
    AE_SUCCESS
}

/// `AdsSetRightsChecking`: taken but not acted on. It controls the client-side
/// existence/rights pre-checks (off by default since v10); the server enforces a
/// dictionary's access rights regardless (see the engine's access checks).
#[unsafe(no_mangle)]
pub extern "system" fn AdsSetRightsChecking(options: Unsigned32) -> Unsigned32 {
    let _call = Call::begin("AdsSetRightsChecking");
    let _ = options;
    AE_SUCCESS
}

/// `AdsSetSQLTimeout`: taken; statements run to their end.
#[unsafe(no_mangle)]
pub extern "system" fn AdsSetSQLTimeout(handle: AdsHandle, timeout: Unsigned32) -> Unsigned32 {
    let _call = Call::begin("AdsSetSQLTimeout");
    let _ = (handle, timeout);
    AE_SUCCESS
}

/// A callback of the client's, as ACE declares them.
pub type AbortCallback = Option<unsafe extern "system" fn() -> Unsigned32>;

/// A progress callback (of the percentage only) of the client's, as ACE declares them.
pub type PercentCallback = Option<unsafe extern "system" fn(Unsigned16) -> Unsigned32>;

/// `AdsRegisterSQLAbortFunc`: taken; tflip never asks whether to abort.
#[unsafe(no_mangle)]
pub extern "system" fn AdsRegisterSQLAbortFunc(callback: AbortCallback) -> Unsigned32 {
    let _call = Call::begin("AdsRegisterSQLAbortFunc");
    let _ = callback;
    AE_SUCCESS
}

/// `AdsClearSQLAbortFunc`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsClearSQLAbortFunc() -> Unsigned32 {
    let _call = Call::begin("AdsClearSQLAbortFunc");
    AE_SUCCESS
}

/// `AdsRegisterProgressCallback`: taken; tflip reports no progress.
#[unsafe(no_mangle)]
pub extern "system" fn AdsRegisterProgressCallback(callback: PercentCallback) -> Unsigned32 {
    let _call = Call::begin("AdsRegisterProgressCallback");
    let _ = callback;
    AE_SUCCESS
}

/// `AdsClearProgressCallback`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsClearProgressCallback() -> Unsigned32 {
    let _call = Call::begin("AdsClearProgressCallback");
    AE_SUCCESS
}

/// What the original engine fails `AdsNullTerminateStrings` with, whatever
/// it is given (not in `ace.h`).
pub(crate) const NULL_TERMINATE_REFUSED: Unsigned32 = 5120;

/// `AdsNullTerminateStrings`: fails as the original engine does, which
/// always terminates strings.
#[unsafe(no_mangle)]
pub extern "system" fn AdsNullTerminateStrings(terminate: Unsigned16) -> Unsigned32 {
    let _call = Call::begin("AdsNullTerminateStrings");
    let _ = terminate;
    fail(NULL_TERMINATE_REFUSED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AE_INSUFFICIENT_BUFFER, fixtures};
    use std::ffi::CStr;

    /// Property 87 answers as the original engine does with its AES
    /// libraries (`tools/ace/sql/props.txt`, run with the patched DLLs):
    /// 1 on any valid handle, 0 handle included; a null pointer is fine.
    #[test]
    fn strong_encryption_is_available() {
        let mut available: Unsigned32 = 0xdead_beef;
        let rc = unsafe { AdsSetProperty(0, 87, std::ptr::from_mut(&mut available) as usize) };
        assert_eq!((rc, available), (AE_SUCCESS, 1));
        assert_eq!(unsafe { AdsSetProperty(0, 87, 0) }, AE_SUCCESS);
    }

    /// Property 27 is accepted on handle 0, as the original engine does.
    #[test]
    fn cached_error_codes_setting_is_accepted() {
        assert_eq!(unsafe { AdsSetProperty(0, 27, 1) }, AE_SUCCESS);
        assert_eq!(unsafe { AdsSetProperty(0, 27, 0) }, AE_SUCCESS);
    }

    #[test]
    fn version_truncates_description() {
        let (mut major, mut minor, mut letter) = (0, 0, 0);
        let mut desc = [0xffu8; 6];
        let mut len: Unsigned16 = desc.len() as Unsigned16;
        let rc = unsafe {
            AdsGetVersion(
                &mut major,
                &mut minor,
                &mut letter,
                desc.as_mut_ptr(),
                &mut len,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!((major, minor), (12, 0));
        assert_eq!(&desc, b"tflip\0");
        assert_eq!(len, 5);
    }

    #[test]
    fn collation_names_are_checked_as_the_engine_does() {
        let repository =
            crate::config::collation_dir().is_some_and(|dir| dir.join("adscollate.adt").exists());
        let check = |name: &str, statement| check_collation(name.to_owned(), statement);
        assert_eq!(check("", false), Ok(String::new()));
        // Names that need no repository.
        assert_eq!(check(":en_US", true), Ok(":en_US".to_owned()));
        assert_eq!(check("oem", true), Ok("oem".to_owned()));
        assert_eq!(check(":xx_YY", true), Err(crate::AE_COLLATOR_NOT_OPENED));
        assert_eq!(check(":", true), Err(crate::AE_COLLATOR_NOT_OPENED));
        if !repository {
            return;
        }
        assert_eq!(
            check("german_vfp_ci_as_1252\t", false),
            Ok("GERMAN_VFP_CI_AS_1252".to_owned())
        );
        for refused in ["GERMAN", " GERMAN_VFP_CI_AS_1252", "ANSI", ":en_US", "oem "] {
            assert_eq!(
                check(refused, false),
                Err(crate::AE_COLLATION_NAME_NOT_FOUND),
                "{refused:?}"
            );
        }
        assert_eq!(
            check("ansi ", true),
            Err(crate::AE_COLLATION_NAME_NOT_FOUND)
        );
        let (mut code, mut buf, mut len) = (0, [0u8; 200], 200);
        unsafe { crate::last_error::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
        assert_eq!(
            &buf[..usize::from(len)],
            b"Error 7147:  The collation name could not be found in the repository.  ansi "
        );
    }

    #[test]
    fn date_format_reads_back_as_the_engine_keeps_it() {
        let _settings = crate::fixtures::settings_lock();
        let get = |size| {
            let mut buf = [b'#'; 12];
            let mut len = size;
            let rc = unsafe { AdsGetDateFormat(buf.as_mut_ptr(), &mut len) };
            (rc, len, buf)
        };
        let mut format = *b"dd.mm.yyyy\0";
        assert_eq!(unsafe { AdsSetDateFormat(format.as_mut_ptr()) }, AE_SUCCESS);
        let (rc, len, buf) = get(12);
        assert_eq!(
            (rc, len, &buf[..11]),
            (AE_SUCCESS, 10, &b"DD.MM.CCYY\0"[..])
        );
        let (rc, len, buf) = get(5);
        assert_eq!(
            (rc, len, &buf[..6]),
            (crate::AE_INSUFFICIENT_BUFFER, 11, &b"DD.MM#"[..])
        );
        let rc = unsafe { AdsGetDateFormat(std::ptr::null_mut(), &mut 12) };
        assert_eq!(rc, AE_INVALID_POINTER);
        // Windows' short date pattern, which ARC passes, is refused.
        let rc = unsafe { AdsSetDateFormat(c"M/d/yyyy".as_ptr().cast_mut().cast()) };
        assert_eq!(rc, crate::AE_INVALID_DATEFORMAT);
        assert_eq!(
            unsafe { crate::AdsSetDateFormat(c"MM/DD/CCYY".as_ptr().cast_mut().cast()) },
            crate::AE_SUCCESS
        );
    }

    #[test]
    fn decimals_round_trip() {
        let mut decimals = 0xffff;
        assert_eq!(unsafe { AdsGetDecimals(&mut decimals) }, AE_SUCCESS);
        assert_eq!(decimals, 2);
        assert_eq!(AdsSetDecimals(5), AE_SUCCESS);
        assert_eq!(unsafe { AdsGetDecimals(&mut decimals) }, AE_SUCCESS);
        assert_eq!(decimals, 5);
        assert_eq!(AdsSetDecimals(2), AE_SUCCESS);
        let rc = unsafe { AdsGetDecimals(std::ptr::null_mut()) };
        assert_eq!(rc, AE_INVALID_POINTER);
    }

    #[test]
    fn show_deleted_round_trip() {
        let _settings = crate::fixtures::settings_lock();
        let mut shown = 0xffff;
        assert_eq!(AdsShowDeleted(ADS_FALSE), AE_SUCCESS);
        assert_eq!(unsafe { AdsGetDeleted(&mut shown) }, AE_SUCCESS);
        assert_eq!(shown, ADS_FALSE);
        assert_eq!(AdsShowDeleted(ADS_TRUE), AE_SUCCESS);
        assert_eq!(unsafe { AdsGetDeleted(&mut shown) }, AE_SUCCESS);
        assert_eq!(shown, ADS_TRUE);
    }

    /// A connection's own date format, as the original engine keeps it:
    /// its tables use it, and setting the process-wide one replaces it.
    #[test]
    fn date_formats_per_connection() {
        let _settings = crate::fixtures::settings_lock();
        let (own, table) = crate::fixtures::open("all_types", c"all_types.adt");
        let (other, other_table) = crate::fixtures::open("all_types", c"all_types.adt");
        let date = |table| {
            let mut buf = [0u8; 16];
            let mut len = buf.len() as u32;
            let field = c"c_date".as_ptr().cast_mut().cast();
            assert_eq!(
                unsafe { crate::AdsGetField(table, field, buf.as_mut_ptr(), &mut len, 0) },
                0
            );
            String::from_utf8(buf[..len as usize].to_vec()).unwrap()
        };
        let format_of = |connect| {
            let mut buf = [0u8; 16];
            let mut len = buf.len() as u16;
            let rc = unsafe { AdsGetDateFormat60(connect, buf.as_mut_ptr(), &mut len) };
            (
                rc,
                String::from_utf8(buf[..usize::from(len)].to_vec()).unwrap(),
            )
        };
        let mut format = *b"dd.mm.yyyy\0";
        assert_eq!(
            unsafe { AdsSetDateFormat60(own, format.as_mut_ptr()) },
            AE_SUCCESS
        );
        assert_eq!(format_of(own), (AE_SUCCESS, "DD.MM.CCYY".to_owned()));
        assert_eq!(format_of(other), (AE_SUCCESS, "MM/DD/CCYY".to_owned()));
        assert_eq!(format_of(0), (AE_SUCCESS, "MM/DD/CCYY".to_owned()));
        assert_eq!(
            (date(table), date(other_table)),
            ("08.11.2017".into(), "11/08/2017".into())
        );
        // The process-wide format replaces every connection's own.
        let mut format = *b"YYYY-MM-DD\0";
        assert_eq!(unsafe { AdsSetDateFormat(format.as_mut_ptr()) }, AE_SUCCESS);
        assert_eq!(format_of(own), (AE_SUCCESS, "CCYY-MM-DD".to_owned()));
        assert_eq!(date(table), "2017-11-08");
        let mut format = *b"M/D/YYYY\0";
        let rc = unsafe { AdsSetDateFormat60(own, format.as_mut_ptr()) };
        assert_eq!(rc, crate::AE_INVALID_DATEFORMAT);
        let rc = unsafe { AdsSetDateFormat60(table, format.as_mut_ptr()) };
        assert_eq!(rc, crate::AE_INVALID_HANDLE);
        assert_eq!(format_of(table).0, crate::AE_INVALID_HANDLE);
        assert_eq!(
            unsafe { crate::AdsSetDateFormat(c"MM/DD/CCYY".as_ptr().cast_mut().cast()) },
            crate::AE_SUCCESS
        );
        assert_eq!(crate::AdsDisconnect(own), crate::AE_SUCCESS);
        assert_eq!(crate::AdsDisconnect(other), crate::AE_SUCCESS);
    }

    #[test]
    fn callbacks_with_64_bit_ids() {
        unsafe extern "system" fn progress(_percent: Unsigned16, _id: i64) -> Unsigned32 {
            0
        }
        assert_eq!(
            AdsRegisterCallbackFunction101(Some(progress), 1 << 40),
            AE_SUCCESS
        );
        assert_eq!(AdsRegisterCallbackFunction101(None, 1), AE_INVALID_POINTER);
        assert_eq!(AdsClearCallbackFunction(), AE_SUCCESS);
    }

    /// The "exact" setting as the original engine keeps it: a global one that
    /// sets everything, and per connection and table.
    #[test]
    fn exact_settings() {
        let _settings = crate::fixtures::settings_lock();
        let (connect, table) = crate::fixtures::open("all_types", c"all_types.adt");
        let (other, other_table) = crate::fixtures::open("all_types", c"all_types.adt");
        let exact_of = |obj| {
            let mut exact = 9;
            let rc = unsafe { AdsGetExact22(obj, &mut exact) };
            (rc == AE_SUCCESS).then_some(exact)
        };
        let all = || [0, connect, table, other, other_table].map(exact_of);
        assert_eq!(all(), [Some(ADS_FALSE); 5]);
        assert_eq!(AdsSetExact22(table, 1), AE_SUCCESS);
        assert_eq!(AdsSetExact22(other, 7), AE_SUCCESS);
        // A connection's setting is not its open tables'.
        assert_eq!(all(), [Some(0), Some(0), Some(1), Some(1), Some(0)]);
        let mut name = *b"all_types.adt\0";
        let (mut opened, null) = (0, std::ptr::null_mut());
        let rc = unsafe {
            crate::AdsOpenTable(other, name.as_mut_ptr(), null, 3, 1, 1, 2, 0, &mut opened)
        };
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!(
            exact_of(opened),
            Some(ADS_TRUE),
            "a new table starts with its connection's"
        );
        // The global setting sets them all.
        assert_eq!(AdsSetExact(5), AE_SUCCESS);
        assert_eq!(all(), [Some(ADS_TRUE); 5]);
        assert_eq!(AdsSetExact22(0, 0), AE_SUCCESS);
        assert_eq!(all(), [Some(ADS_FALSE); 5]);
        let mut exact = 9;
        assert_eq!(unsafe { AdsGetExact(&mut exact) }, AE_SUCCESS);
        assert_eq!(exact, ADS_FALSE);
        assert_eq!(AdsSetExact22(0x12345, 1), crate::AE_INVALID_HANDLE);
        let rc = unsafe { AdsGetExact22(0x12345, &mut exact) };
        assert_eq!(
            rc, AE_INVALID_CONNECTION_HANDLE,
            "not AE_INVALID_HANDLE, as in the original"
        );
        assert_eq!(
            unsafe { AdsGetExact22(table, std::ptr::null_mut()) },
            AE_INVALID_POINTER
        );
        assert_eq!(
            unsafe { AdsGetExact(std::ptr::null_mut()) },
            AE_INVALID_POINTER
        );
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
        assert_eq!(crate::AdsDisconnect(other), crate::AE_SUCCESS);
    }

    fn eval(table: AdsHandle, expression: &CStr) -> String {
        let (mut buf, mut len) = ([0u8; 64], 64);
        let expression = expression.as_ptr().cast_mut().cast();
        let rc = unsafe { crate::AdsEvalStringExpr(table, expression, buf.as_mut_ptr(), &mut len) };
        assert_eq!(rc, AE_SUCCESS);
        String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned()
    }

    /// As the original engine answers (probed).
    #[test]
    fn epochs_and_search_paths() {
        let _settings = fixtures::settings_lock();
        let (connect, table) = fixtures::open("all_types", c"all_types.adt");
        let mut epoch = 0;
        assert_eq!(unsafe { AdsGetEpoch(&mut epoch) }, AE_SUCCESS);
        assert_eq!(epoch, 1900);
        assert_eq!(eval(table, c"DTOS(CTOD('01/01/49'))"), "19490101");
        assert_eq!(AdsSetEpoch(1950), AE_SUCCESS);
        assert_eq!(eval(table, c"DTOS(CTOD('01/01/49'))"), "20490101");
        assert_eq!(eval(table, c"DTOS(CTOD('01/01/50'))"), "19500101");
        assert_eq!(AdsSetEpoch(1900), AE_SUCCESS);
        assert_eq!(
            unsafe { AdsGetEpoch(std::ptr::null_mut()) },
            AE_INVALID_POINTER
        );

        let (mut buf, mut len) = ([0u8; 32], 32);
        unsafe { AdsSetSearchPath(c"C:\\a;D:\\b".as_ptr().cast_mut().cast()) };
        assert_eq!(
            unsafe { AdsGetSearchPath(buf.as_mut_ptr(), &mut len) },
            AE_SUCCESS
        );
        assert_eq!(&buf[..usize::from(len)], b"C:\\a;D:\\b");
        len = 3;
        let rc = unsafe { AdsGetSearchPath(buf.as_mut_ptr(), &mut len) };
        assert_eq!((rc, len), (AE_INSUFFICIENT_BUFFER, 10));
        unsafe { AdsSetSearchPath(std::ptr::null_mut()) };

        let mut len = 32;
        assert_eq!(
            unsafe { AdsGetDefault(buf.as_mut_ptr(), &mut len) },
            AE_SUCCESS
        );
        assert_eq!(len, 0);
        unsafe { AdsSetDefault(c"c:\\data\\x".as_ptr().cast_mut().cast()) };
        len = 32;
        assert_eq!(
            unsafe { AdsGetDefault(buf.as_mut_ptr(), &mut len) },
            AE_SUCCESS
        );
        assert_eq!(&buf[..usize::from(len)], b"c:\\data\\x\\");
        len = 3;
        let rc = unsafe { AdsGetDefault(buf.as_mut_ptr(), &mut len) };
        assert_eq!((rc, len), (AE_INSUFFICIENT_BUFFER, 11));
        unsafe { AdsSetDefault(c"".as_ptr().cast_mut().cast()) };
        len = 32;
        assert_eq!(
            unsafe { AdsGetDefault(buf.as_mut_ptr(), &mut len) },
            AE_SUCCESS
        );
        assert_eq!(len, 0);
        unsafe { AdsSetDefault(c"rel\\dir".as_ptr().cast_mut().cast()) };
        assert_eq!(AdsClearDefault(), AE_SUCCESS);
        len = 32;
        assert_eq!(
            unsafe { AdsGetDefault(buf.as_mut_ptr(), &mut len) },
            AE_SUCCESS
        );
        assert_eq!(len, 0);
        crate::AdsDisconnect(connect);
    }

    crate::fixtures::remote_twins!(
        date_formats_per_connection,
        exact_settings,
        epochs_and_search_paths
    );
}
