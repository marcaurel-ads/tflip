//! The crossings of the C ABI that the exports share: reading the strings
//! and buffers a client passes, and writing results back to its buffers.
//! Each relies on the contract the ACE API sets for the pointer, which the
//! export's `# Safety` section repeats; the exports read their arguments
//! through these and leave the rest of the work to safe code.

use std::ffi::CStr;
use std::path::PathBuf;

use crate::last_error::{fail, null_param};
use crate::{
    AE_INSUFFICIENT_BUFFER, AE_INVALID_POINTER, AE_SUCCESS, Unsigned16, Unsigned32, Wchar, ansi,
};

/// The bytes of the NUL-terminated string at `text`, without the NUL;
/// `None` for null.
///
/// # Safety
///
/// `text` must be null or point to a NUL-terminated string that is not
/// changed while the bytes are borrowed.
pub(crate) unsafe fn c_str<'a>(text: *const u8) -> Option<&'a [u8]> {
    // SAFETY: not null, so a NUL-terminated string, as the caller promises.
    (!text.is_null()).then(|| unsafe { CStr::from_ptr(text.cast()) }.to_bytes())
}

/// The NUL-terminated ANSI string at `text`, as a Rust string; `None` for
/// null.
///
/// # Safety
///
/// `text` must be null or point to a NUL-terminated string.
pub(crate) unsafe fn text(text: *const u8) -> Option<String> {
    // SAFETY: as the caller promises.
    unsafe { c_str(text) }.map(ansi::to_string)
}

/// The NUL-terminated UTF-16 string at `text`.
///
/// # Safety
///
/// `text` must point to a NUL-terminated UTF-16 string.
pub(crate) unsafe fn wide_string(text: *const Wchar) -> String {
    let mut units = Vec::new();
    for i in 0.. {
        // SAFETY: the units up to the NUL are the caller's string.
        match unsafe { *text.add(i) } {
            0 => break,
            unit => units.push(unit),
        }
    }
    String::from_utf16_lossy(&units)
}

/// The file name at `file`, an ANSI string, as a path; `None` for null.
///
/// # Safety
///
/// `file` must be null or point to a NUL-terminated string.
pub(crate) unsafe fn path_arg(file: *const u8) -> Option<PathBuf> {
    // SAFETY: as the caller promises.
    unsafe { c_str(file) }.map(|file| ansi::to_os(file).into())
}

/// The file name at `file`, a UTF-16 string, as a path; `None` for null.
///
/// # Safety
///
/// `file` must be null or point to a NUL-terminated UTF-16 string.
pub(crate) unsafe fn wide_path_arg(file: *const Wchar) -> Option<PathBuf> {
    // SAFETY: not null, so a NUL-terminated wide string, as the caller
    // promises.
    (!file.is_null()).then(|| unsafe { wide_string(file) }.into())
}

/// The `len` items at `buf`; `None` for null.
///
/// # Safety
///
/// `buf` must be null or hold `len` initialized items that are not changed
/// while they are borrowed.
pub(crate) unsafe fn slice<'a, T>(buf: *const T, len: usize) -> Option<&'a [T]> {
    // SAFETY: not null, so `len` items, as the caller promises.
    (!buf.is_null()).then(|| unsafe { std::slice::from_raw_parts(buf, len) })
}

/// A length the client passes with a buffer: 16 bits in most calls, 32 in
/// the later ones.
pub(crate) trait BufLen: Copy {
    /// The length of `n` units, as large as the type goes.
    fn saturating_from(n: usize) -> Self;
    fn units(self) -> usize;
    fn saturating_inc(self) -> Self;
}

macro_rules! buf_len {
    ($($type:ty),*) => {$(
        impl BufLen for $type {
            fn saturating_from(n: usize) -> Self {
                Self::try_from(n).unwrap_or(Self::MAX)
            }

            fn units(self) -> usize {
                self as usize
            }

            fn saturating_inc(self) -> Self {
                self.saturating_add(1)
            }
        }
    )*};
}

buf_len!(Unsigned16, Unsigned32);

/// Returns `src` through a caller's buffer of `*len` bytes, NUL-terminated,
/// and sets `*len` to its length without the NUL. If it does not fit, the
/// buffer is left alone, `*len` is set to the length `src` needs (again
/// without the NUL) and `AE_INSUFFICIENT_BUFFER` is returned.
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
pub(crate) unsafe fn write_string<L: BufLen>(src: &[u8], buf: *mut u8, len: *mut L) -> Unsigned32 {
    if buf.is_null() || len.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    let needed = L::saturating_from(src.len());
    // SAFETY: neither is null, so `len` is readable and writable and `buf`
    // holds `*len` bytes, which the copy and the NUL stay inside.
    unsafe {
        if src.len() >= (*len).units() {
            *len = needed;
            return fail(AE_INSUFFICIENT_BUFFER);
        }
        std::ptr::copy_nonoverlapping(src.as_ptr(), buf, src.len());
        *buf.add(src.len()) = 0;
        *len = needed;
    }
    AE_SUCCESS
}

/// [`write_string`] with `buf` the call's argument number `param` and `len`
/// the one after it: a NULL one fails naming its number
/// (`fixtures/errors/param_n.json`).
///
/// # Safety
///
/// As for [`write_string`].
pub(crate) unsafe fn write_string_at<L: BufLen>(
    src: &[u8],
    buf: *mut u8,
    len: *mut L,
    param: u32,
) -> Unsigned32 {
    if let Some(code) = null_buffer(buf, len, param) {
        return code;
    }
    // SAFETY: as the caller promises, and neither is null.
    unsafe { write_string(src, buf, len) }
}

/// [`write_cut`] with the numbers of the pointers named, as
/// [`write_string_at`] does.
///
/// # Safety
///
/// As for [`write_string`].
pub(crate) unsafe fn write_cut_at<L: BufLen>(
    src: &[u8],
    buf: *mut u8,
    len: *mut L,
    param: u32,
) -> Unsigned32 {
    if let Some(code) = null_buffer(buf, len, param) {
        return code;
    }
    // SAFETY: as the caller promises, and neither is null.
    unsafe { write_cut(src, buf, len) }
}

/// The failure of a buffer `buf` (argument number `param`) and its length
/// `len` (the next one) if either is NULL, the buffer first.
pub(crate) fn null_buffer<T, L>(buf: *mut T, len: *mut L, param: u32) -> Option<Unsigned32> {
    if buf.is_null() {
        Some(null_param(param))
    } else if len.is_null() {
        Some(null_param(param + 1))
    } else {
        None
    }
}

/// Returns `src` like [`write_string`], except that what does not fit is
/// copied as far as it fits (without a NUL) and reports the size it needs
/// including the NUL, as the original engine does for scope keys and the
/// date format.
///
/// # Safety
///
/// As for [`write_string`].
pub(crate) unsafe fn write_cut<L: BufLen>(src: &[u8], buf: *mut u8, len: *mut L) -> Unsigned32 {
    if buf.is_null() || len.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: neither is null, as `write_units` needs.
    unsafe { write_units(src, buf, len) }
}

/// Returns `name` like [`write_string`], except that a name that does not fit
/// reports the size it needs including the NUL, as the original engine does
/// for table and field names.
///
/// # Safety
///
/// As for [`write_string`].
pub(crate) unsafe fn write_name<L: BufLen>(name: &[u8], buf: *mut u8, len: *mut L) -> Unsigned32 {
    // SAFETY: as the caller promises.
    let rc = unsafe { write_string(name, buf, len) };
    if rc == AE_INSUFFICIENT_BUFFER {
        // SAFETY: `write_string` only reports a short buffer for a `len`
        // that is not null.
        unsafe { *len = (*len).saturating_inc() };
    }
    rc
}

/// Writes `units` and a 0 to `buf` of `*len` units as [`write_cut`] does
/// with bytes, for units that are bytes or UTF-16 code units, and for the
/// callers that check their pointers before they have the units.
///
/// # Safety
///
/// `buf` must hold `*len` units; `len` must be valid for reads and writes.
pub(crate) unsafe fn write_units<T: Copy + Default, L: BufLen>(
    units: &[T],
    buf: *mut T,
    len: *mut L,
) -> Unsigned32 {
    let needed = L::saturating_from(units.len());
    // SAFETY: `len` is readable and writable and `buf` holds `*len` units,
    // which the copies and the 0 stay inside, as the caller promises.
    unsafe {
        if needed.units() >= (*len).units() {
            std::ptr::copy_nonoverlapping(units.as_ptr(), buf, units.len().min((*len).units()));
            *len = needed.saturating_inc();
            return fail(AE_INSUFFICIENT_BUFFER);
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), buf, units.len());
        *buf.add(units.len()) = T::default();
        *len = needed;
    }
    AE_SUCCESS
}
