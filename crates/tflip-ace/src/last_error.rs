//! The last error of each thread, for `AdsGetLastError`.
//!
//! As in the original engine, every export begins a [`Call`], which clears
//! the last error, and records the code of a failure with [`fail`]. A
//! failure of the handle check, which the original engine does before it
//! clears the last error, is recorded with [`fail_handle`] and does not
//! replace an error that was pending. `AdsGetLastError` and
//! `AdsGetDateFormat` do not clear it.

use std::cell::{Cell, RefCell};

use tflip_engine::ace_error::describe;
pub(crate) use tflip_engine::ace_error::{
    NOT_A_CONNECTION, NOT_A_HANDLE, NOT_A_STATEMENT, NOT_A_TABLE, error_text,
};

use crate::{AE_INVALID_POINTER, AE_SUCCESS, Unsigned8, Unsigned16, Unsigned32, write_string};

thread_local! {
    static LAST_ERROR: Cell<Unsigned32> = const { Cell::new(AE_SUCCESS) };
    /// The last error when the outermost call began.
    static PENDING: Cell<Unsigned32> = const { Cell::new(AE_SUCCESS) };
    /// How many calls are running on this thread (exports call each other).
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    /// The export the outermost call is, for the `err` log channel.
    static EXPORT: Cell<&'static str> = const { Cell::new("") };
    /// The text of an error that has its own, with its code: SQL errors, and
    /// those the original engine adds details to.
    static TEXT: RefCell<Option<(Unsigned32, String)>> = const { RefCell::new(None) };
    /// The text of the last error when the outermost call began.
    static PENDING_TEXT: RefCell<Option<(Unsigned32, String)>> = const { RefCell::new(None) };
    /// What the `err` log channel adds about the running export's failure.
    static DETAIL: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// A call of an export, from its start to its return: the outermost one
/// clears the last error when it begins.
pub(crate) struct Call(());

impl Call {
    /// Begins a call of the export `name`.
    pub(crate) fn begin(name: &'static str) -> Call {
        if DEPTH.get() == 0 {
            PENDING.set(LAST_ERROR.replace(AE_SUCCESS));
            PENDING_TEXT.set(TEXT.take());
            EXPORT.set(name);
            DETAIL.set(None);
        }
        DEPTH.set(DEPTH.get() + 1);
        Call(())
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        DEPTH.set(DEPTH.get() - 1);
    }
}

/// Runs `f`, a call into a library that calls the exports itself (an
/// external procedure), as if no export were running: its calls clear and
/// set the last error as outermost ones do, and the running export's is as
/// it was once `f` returns.
pub(crate) fn outside<R>(f: impl FnOnce() -> R) -> R {
    let depth = DEPTH.replace(0);
    let saved = (LAST_ERROR.get(), PENDING.get(), EXPORT.get());
    let texts = (TEXT.take(), PENDING_TEXT.take(), DETAIL.take());
    let result = f();
    DEPTH.set(depth);
    LAST_ERROR.set(saved.0);
    PENDING.set(saved.1);
    EXPORT.set(saved.2);
    TEXT.set(texts.0);
    PENDING_TEXT.set(texts.1);
    DETAIL.set(texts.2);
    result
}

/// Clears this thread's last error (an export that succeeds after a step
/// that failed).
pub(crate) fn clear() {
    LAST_ERROR.set(AE_SUCCESS);
}

/// Records `code` with `text`, what `AdsGetLastError` says about it, as
/// this thread's last error; returns `code`.
pub(crate) fn fail_text(code: Unsigned32, text: String) -> Unsigned32 {
    report(code);
    LAST_ERROR.set(code);
    TEXT.set(Some((code, text)));
    code
}

/// Adds `detail` after the original engine's text of `code`, the failure
/// just recorded, as the original engine adds it (the field a setter
/// refused); returns `code`.
pub(crate) fn add_detail(code: Unsigned32, detail: &str) -> Unsigned32 {
    if LAST_ERROR.get() == code
        && let Some(text) = error_text(code)
    {
        TEXT.set(Some((code, format!("Error {code}:  {text}  {detail}"))));
    }
    code
}

/// Records `code` with `detail` after the original engine's text of it, as
/// the original engine adds it (`Parameter 3`, the file it did not find,
/// ...); returns `code`.
pub(crate) fn fail_detail(code: Unsigned32, detail: &str) -> Unsigned32 {
    let text = error_text(code).unwrap_or_default();
    fail_text(code, format!("Error {code}:  {text}  {detail}"))
}

/// Puts back the last error that was there when the outermost call began,
/// for a call that the original engine leaves without a trace.
pub(crate) fn restore_pending() {
    if DEPTH.get() > 0 {
        LAST_ERROR.set(PENDING.get());
        TEXT.set(PENDING_TEXT.take());
    }
}

/// Fails with `AE_INVALID_POINTER` and the number of the argument that was
/// NULL (`Parameter 3`), as the original engine's text names it.
pub(crate) fn null_param(number: u32) -> Unsigned32 {
    fail_detail(AE_INVALID_POINTER, &format!("Parameter {number}"))
}

/// Records `code` as this thread's last error, unless it is `AE_SUCCESS`,
/// and returns it.
pub(crate) fn fail(code: Unsigned32) -> Unsigned32 {
    report(code);
    if code != AE_SUCCESS {
        LAST_ERROR.set(code);
    }
    code
}

/// Records `code`, the failure of a call's handle check, unless an error
/// was pending when the call began, which stays; returns `code`.
pub(crate) fn fail_handle(code: Unsigned32) -> Unsigned32 {
    report(code);
    let pending = if DEPTH.get() > 0 {
        PENDING.get()
    } else {
        LAST_ERROR.get()
    };
    LAST_ERROR.set(if pending != AE_SUCCESS { pending } else { code });
    if pending != AE_SUCCESS && DEPTH.get() > 0 {
        TEXT.set(PENDING_TEXT.take());
    }
    code
}

/// [`fail_handle`], with `detail` after the original engine's text of
/// `code` when it is recorded.
pub(crate) fn fail_handle_detail(code: Unsigned32, detail: &str) -> Unsigned32 {
    fail_handle(code);
    let pending = if DEPTH.get() > 0 {
        PENDING.get()
    } else {
        AE_SUCCESS
    };
    if pending == AE_SUCCESS
        && let Some(text) = error_text(code)
    {
        TEXT.set(Some((code, format!("Error {code}:  {text}  {detail}"))));
    }
    code
}

/// Reports a failure of the running export on the `err` log channel.
fn report(code: Unsigned32) {
    if code != AE_SUCCESS {
        let detail = DETAIL
            .take()
            .map_or_else(String::new, |d| format!(" ({d})"));
        crate::log::err(format_args!(
            "{}: {code}: {}{detail}",
            EXPORT.get(),
            describe(code)
        ));
    }
}

/// Adds `detail` to what the `err` log channel reports if the running
/// export fails, such as the argument it refused.
pub(crate) fn detail(detail: String) {
    DETAIL.set(Some(detail));
}

/// Runs `f`, the export `name` that does not clear the last error: what it
/// records replaces only no error.
pub(crate) fn keeping_pending(name: &'static str, f: impl FnOnce() -> Unsigned32) -> Unsigned32 {
    if DEPTH.get() == 0 {
        EXPORT.set(name);
        DETAIL.set(None);
    }
    let pending = LAST_ERROR.get();
    let code = f();
    if pending != AE_SUCCESS {
        LAST_ERROR.set(pending);
    }
    code
}

/// `AdsGetLastError`: the code of this thread's last error (0 if none), and
/// a description in `buf` of `*len` bytes; `*len` is set to its length.
///
/// The description is what the failure recorded (an SQL error's text, or a
/// text with details), else the original engine's text for the code, as
/// `Error 5004:  <text>`; only a code it has no text for gets tflip's own
/// words.
///
/// # Safety
///
/// `code` must be null or valid for writes; `buf` must be null or hold `*len`
/// bytes; `len` must be null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetLastError(
    code: *mut Unsigned32,
    buf: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    if code.is_null() {
        return AE_INVALID_POINTER;
    }
    let last = LAST_ERROR.get();
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *code = last };
    if buf.is_null() || len.is_null() {
        return AE_SUCCESS;
    }
    let own = TEXT.with_borrow(|text| match text {
        Some((code, text)) if *code == last => Some(text.clone()),
        _ => None,
    });
    // As in the original engine, no error writes nothing.
    if last == AE_SUCCESS {
        return AE_SUCCESS;
    }
    let text = own.unwrap_or_else(|| match error_text(last) {
        Some(text) => format!("Error {last}:  {text}"),
        None => format!("Error {last}:  {}", describe(last)),
    });
    let text = crate::ansi::from_str_lossy(&text);
    // SAFETY: `buf` and `len` are as # Safety says.
    unsafe { write_string(&text, buf, len) }
}

/// `AdsGetErrorString`: the original engine's text for the error `code` in
/// `buf` of `*len` bytes; `*len` is set to its length. As in the original
/// engine, a buffer that is too small gets what fits and
/// `AE_INSUFFICIENT_BUFFER`, with `*len` set to the length needed with the
/// NUL, and a code without a text gets `Error code <code> is unknown.` and
/// `AE_STRING_NOT_FOUND`.
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetErrorString(
    code: Unsigned32,
    buf: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetErrorString");
    let (text, known) = match error_text(code) {
        Some(text) => (text.to_owned(), true),
        None => (format!("Error code {code} is unknown."), false),
    };
    let text = crate::ansi::from_str_lossy(&text);
    // SAFETY: `buf` and `len` are as # Safety says.
    let rc = unsafe { crate::write_cut(&text, buf, len) };
    match rc {
        AE_SUCCESS if !known => fail(AE_STRING_NOT_FOUND),
        rc => rc,
    }
}

/// `AdsSetLastError` (unpublished): makes `code`, cut to 16 bits, this
/// thread's last error with `details` after its text, and returns it. As in
/// the original engine, it clears nothing and records only over no error.
///
/// # Safety
///
/// `details` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetLastError(
    code: Unsigned32,
    details: *mut Unsigned8,
) -> Unsigned32 {
    let code = code & 0xffff;
    if code == AE_SUCCESS || LAST_ERROR.get() != AE_SUCCESS {
        return code;
    }
    let mut text = match error_text(code) {
        Some(text) => format!("Error {code}:  {text}"),
        None => format!("Error {code}:  Error code {code} is unknown."),
    };
    // SAFETY: `details` is null or NUL-terminated, as # Safety says.
    if let Some(details) = unsafe { crate::ffi::c_str(details) }
        && !details.is_empty()
    {
        text = format!("{text}  {}", crate::ansi::to_string(details));
    }
    LAST_ERROR.set(code);
    TEXT.set(Some((code, text)));
    code
}

/// `AdsGetErrorString`'s error for a code without a text.
const AE_STRING_NOT_FOUND: Unsigned32 = 5055;

/// `AdsShowError`: shows the last error in a message box titled `title` in
/// the original engine; tflip shows nothing, and logs it on the `err`
/// channel instead.
///
/// # Safety
///
/// `title` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsShowError(_title: *mut Unsigned8) -> Unsigned32 {
    let mut code = 0;
    let (mut buf, mut len) = ([0u8; 600], 600);
    // SAFETY: `buf` holds `len` bytes; `code` and `len` are local.
    unsafe { crate::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
    if code == AE_SUCCESS {
        return AE_SUCCESS;
    }
    crate::log::err(format_args!(
        "AdsShowError: {}",
        String::from_utf8_lossy(&buf[..usize::from(len).min(600)])
    ));
    AE_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AE_INVALID_TABLE_HANDLE, AdsGetRecordLength};

    #[test]
    fn last_errors() {
        let mut code = 0xffff_ffff;
        let mut buf = [0u8; 200];
        let mut len = buf.len() as u16;
        let rc = unsafe { AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
        // As in the original engine, no error writes nothing.
        assert_eq!(
            (rc, code, len),
            (AE_SUCCESS, 0, 200),
            "no error yet on this thread"
        );

        let mut length = 0;
        let rc = unsafe { AdsGetRecordLength(0x1234_5678, &mut length) };
        assert_eq!(rc, AE_INVALID_TABLE_HANDLE);
        let mut len = buf.len() as u16;
        let rc = unsafe { AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
        assert_eq!((rc, code), (AE_SUCCESS, AE_INVALID_TABLE_HANDLE));
        // With the detail the original engine adds.
        assert_eq!(
            &buf[..usize::from(len)],
            b"Error 5023:  Advantage does not have a table or cursor open with the specified \
              handle.  The handle must be to a table."
        );

        // A successful call clears it.
        assert_eq!(crate::AdsCacheOpenCursors(25), AE_SUCCESS);
        assert_eq!(last(), AE_SUCCESS);
    }

    /// The original engine's texts, as `fixtures/errors/texts.json` has
    /// them.
    #[test]
    fn error_strings() {
        let get = |code, size| {
            let (mut buf, mut len) = ([0x5au8; 200], size);
            let rc = unsafe { AdsGetErrorString(code, buf.as_mut_ptr(), &mut len) };
            (
                rc,
                len,
                String::from_utf8_lossy(&buf[..usize::from(len).min(size.into())]).into_owned(),
            )
        };
        assert_eq!(
            get(0, 200),
            (AE_SUCCESS, 18, "No error occurred.".to_owned())
        );
        assert_eq!(
            get(5021, 200),
            (
                AE_SUCCESS,
                32,
                "The pointer given was not valid.".to_owned()
            )
        );
        assert_eq!(
            get(1, 200),
            (
                AE_STRING_NOT_FOUND,
                24,
                "Error code 1 is unknown.".to_owned()
            )
        );
        assert_eq!(
            get(5004, 5),
            (crate::AE_INSUFFICIENT_BUFFER, 103, "Eithe".to_owned())
        );
        let json = std::fs::read_to_string(crate::fixtures::dir("../errors/texts.json")).unwrap();
        let texts: serde_json::Value = serde_json::from_str(&json).unwrap();
        let known = texts
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, text)| text.is_string())
            .count();
        assert_eq!(tflip_engine::ace_error::TEXTS.len(), known);
    }

    fn last_text() -> String {
        let (mut code, mut buf, mut len) = (0, [0u8; 300], 300);
        unsafe { AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
        String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned()
    }

    /// As the original engine answers (probed).
    #[test]
    fn errors_set_by_clients() {
        let set = |code, details: Option<&std::ffi::CStr>| {
            let details = details.map_or(std::ptr::null_mut(), |d| d.as_ptr().cast_mut().cast());
            unsafe { AdsSetLastError(code, details) }
        };
        assert_eq!(crate::AdsCacheOpenCursors(25), AE_SUCCESS);
        assert_eq!(set(7777, None), 7777);
        assert_eq!(last_text(), "Error 7777:  Error code 7777 is unknown.");
        // Only over no error.
        assert_eq!(set(5004, Some(c"det")), 5004);
        assert_eq!(last(), 7777);
        assert_eq!(crate::AdsCacheOpenCursors(25), AE_SUCCESS);
        assert_eq!(set(0, Some(c"x")), 0);
        assert_eq!(last(), AE_SUCCESS);
        assert_eq!(set(99999, Some(c"x")), 34463);
        assert_eq!(last_text(), "Error 34463:  Error code 34463 is unknown.  x");
        assert_eq!(crate::AdsCacheOpenCursors(25), AE_SUCCESS);
        assert_eq!(set(5004, Some(c"hello")), 5004);
        assert!(last_text().ends_with("to access the file.  hello"));
        assert_eq!(crate::AdsCacheOpenCursors(25), AE_SUCCESS);
    }

    fn last() -> Unsigned32 {
        let mut code = 0xffff_ffff;
        let rc = unsafe { AdsGetLastError(&mut code, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(rc, AE_SUCCESS);
        code
    }

    /// As the original engine records them.
    #[test]
    fn pending_errors() {
        let (connect, table) = crate::fixtures::open("all_types", c"all_types.adt");
        let bad_name = || {
            let (mut buf, mut len) = ([0u8; 16], 16);
            let name = c"nope".as_ptr().cast_mut().cast();
            unsafe { crate::AdsGetField(table, name, buf.as_mut_ptr(), &mut len, 0) }
        };
        let mut length = 0;

        // A failed handle check keeps an error that was pending...
        assert_eq!(bad_name(), crate::AE_INVALID_FIELDNAME);
        let rc = unsafe { AdsGetRecordLength(0x1234_5678, &mut length) };
        assert_eq!(
            (rc, last()),
            (AE_INVALID_TABLE_HANDLE, crate::AE_INVALID_FIELDNAME)
        );
        // ... and is recorded when none was.
        assert_eq!(crate::AdsCacheOpenCursors(25), AE_SUCCESS);
        let rc = unsafe { AdsGetRecordLength(connect, &mut length) };
        assert_eq!(
            (rc, last()),
            (AE_INVALID_TABLE_HANDLE, AE_INVALID_TABLE_HANDLE)
        );
        // Other failures replace it.
        assert_eq!(bad_name(), crate::AE_INVALID_FIELDNAME);
        assert_eq!(last(), crate::AE_INVALID_FIELDNAME);

        // AdsGetDateFormat clears nothing, and records only over no error.
        let (mut format, mut len) = ([0u8; 16], 16u16);
        assert_eq!(
            unsafe { crate::AdsGetDateFormat(format.as_mut_ptr(), &mut len) },
            AE_SUCCESS
        );
        assert_eq!(last(), crate::AE_INVALID_FIELDNAME);
        len = 2;
        let rc = unsafe { crate::AdsGetDateFormat(format.as_mut_ptr(), &mut len) };
        assert_eq!(
            (rc, last()),
            (crate::AE_INSUFFICIENT_BUFFER, crate::AE_INVALID_FIELDNAME)
        );
        assert_eq!(crate::AdsCacheOpenCursors(25), AE_SUCCESS);
        len = 2;
        let rc = unsafe { crate::AdsGetDateFormat(format.as_mut_ptr(), &mut len) };
        assert_eq!(
            (rc, last()),
            (crate::AE_INSUFFICIENT_BUFFER, crate::AE_INSUFFICIENT_BUFFER)
        );

        // AdsGetNullRecord does not record a buffer that is too small.
        let mut record = [0u8; 4];
        let rc = unsafe { crate::AdsGetNullRecord(table, record.as_mut_ptr(), 4) };
        assert_eq!((rc, last()), (crate::AE_INSUFFICIENT_BUFFER, AE_SUCCESS));
        assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
    }
}
