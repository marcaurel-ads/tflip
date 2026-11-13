//! `AdsImageToClipboard`: putting an Image field's bitmap on the Windows
//! clipboard.
//!
//! The original engine takes the BMP stored in an `ADS_IMAGE` field and sets it
//! as `CF_DIB` on the clipboard — the BMP's bytes with its 14-byte
//! `BITMAPFILEHEADER` stripped off (a packed DIB: `BITMAPINFOHEADER`, any colour
//! table, then the pixels). Windows then auto-synthesises `CF_BITMAP` and
//! `CF_DIBV5` from it. The behaviour, the format and the error codes below were
//! all derived black-box from the real 12.0 engine on native Windows (see
//! `tools/ace/clipboard`).
//!
//! The clipboard is a local-Windows concept, so this only does anything when
//! tflip's `ace32.dll` runs inside a Windows process. Built for any other host
//! (tflipd on Linux) it reports `AE_NOT_IMPLEMENTED` for a valid image, exactly
//! as the earlier stub did.

use crate::Unsigned32;

/// A `BITMAPFILEHEADER` is 14 bytes; a `BITMAPINFOHEADER` is 40. A valid BMP
/// has the file header (magic `BM`) followed by at least an info header.
const FILE_HEADER: usize = 14;
const INFO_HEADER: usize = 40;

/// The engine's two error codes here: 5091 "The binary data is not a valid
/// image." and 5001 "A memory allocation failed…" (which it also returns for an
/// internal GDI/bitmap failure).
const AE_INVALID_IMAGE: Unsigned32 = 5091;
const AE_ALLOCATION_FAILED: Unsigned32 = 5001;

/// Why an image is not usable, with the detail the engine appends to the error
/// text after the base message.
#[cfg_attr(test, derive(Debug))]
struct Rejected {
    code: Unsigned32,
    detail: &'static str,
}

/// Validates the stored image as the engine does and, if good, returns the DIB
/// (the BMP with its file header stripped) ready for `CF_DIB`.
///
/// Matches the native-engine probe: an image shorter than a `BITMAPFILEHEADER`
/// is "Missing BITMAPFILEHEADER."; one not starting with `BM` is "Invalid
/// bitmap type."; a `BM` header with no room for a `BITMAPINFOHEADER` fails the
/// engine's bitmap build with 5001 "Unable to allocate memory for bitmap."
fn validate(image: &[u8]) -> Result<&[u8], Rejected> {
    if image.len() < FILE_HEADER {
        return Err(Rejected {
            code: AE_INVALID_IMAGE,
            detail: "Missing BITMAPFILEHEADER.",
        });
    }
    if &image[..2] != b"BM" {
        return Err(Rejected {
            code: AE_INVALID_IMAGE,
            detail: "Invalid bitmap type.",
        });
    }
    if image.len() < FILE_HEADER + INFO_HEADER {
        return Err(Rejected {
            code: AE_ALLOCATION_FAILED,
            detail: "Unable to allocate memory for bitmap.",
        });
    }
    Ok(&image[FILE_HEADER..])
}

/// Records `code` with the engine's text for it plus `detail`, as
/// `AdsGetLastError` reports it (`Error <code>:  <base>  <detail>`), and returns
/// `code`.
fn reject(code: Unsigned32, detail: &str) -> Unsigned32 {
    let base = crate::last_error::error_text(code).unwrap_or_default();
    crate::last_error::fail_text(code, format!("Error {code}:  {base}  {detail}"))
}

/// Validates the Image field's bytes and copies the bitmap to the clipboard.
/// Returns `AE_SUCCESS` or the matching `AE_*` code.
pub(crate) fn copy_image(image: &[u8]) -> Unsigned32 {
    match validate(image) {
        Err(rejected) => reject(rejected.code, rejected.detail),
        Ok(dib) => set_clipboard_dib(dib),
    }
}

/// Puts `dib` on the clipboard as `CF_DIB` (Windows). Any failure of the OS
/// clipboard/allocation calls is reported the way the engine reports its own
/// bitmap failure: 5001 "Unable to allocate memory for bitmap."
#[cfg(windows)]
fn set_clipboard_dib(dib: &[u8]) -> Unsigned32 {
    use sys::*;
    let oom = || {
        reject(
            AE_ALLOCATION_FAILED,
            "Unable to allocate memory for bitmap.",
        )
    };
    // SAFETY: a straightforward CF_DIB clipboard sequence; the global block is
    // handed to the clipboard on success and freed on every failure path.
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, dib.len());
        if mem.is_null() {
            return oom();
        }
        let ptr = GlobalLock(mem);
        if ptr.is_null() {
            GlobalFree(mem);
            return oom();
        }
        core::ptr::copy_nonoverlapping(dib.as_ptr(), ptr.cast::<u8>(), dib.len());
        GlobalUnlock(mem);
        if OpenClipboard(core::ptr::null_mut()) == 0 {
            GlobalFree(mem);
            return oom();
        }
        EmptyClipboard();
        if SetClipboardData(CF_DIB, mem).is_null() {
            // The clipboard did not take ownership; the block is still ours.
            CloseClipboard();
            GlobalFree(mem);
            return oom();
        }
        CloseClipboard();
        crate::AE_SUCCESS
    }
}

/// On any non-Windows host there is no clipboard to copy to, so a valid image
/// reports `AE_NOT_IMPLEMENTED` (an invalid one is still rejected by
/// [`validate`], as that is platform-independent).
#[cfg(not(windows))]
fn set_clipboard_dib(_dib: &[u8]) -> Unsigned32 {
    crate::last_error::fail(crate::AE_NOT_IMPLEMENTED)
}

#[cfg(windows)]
mod sys {
    use core::ffi::c_void;

    pub type Handle = *mut c_void;
    pub const GMEM_MOVEABLE: u32 = 0x0002;
    pub const CF_DIB: u32 = 8;

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn OpenClipboard(hwnd: Handle) -> i32;
        pub fn EmptyClipboard() -> i32;
        pub fn SetClipboardData(format: u32, mem: Handle) -> Handle;
        pub fn CloseClipboard() -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
        pub fn GlobalLock(mem: Handle) -> *mut c_void;
        pub fn GlobalUnlock(mem: Handle) -> i32;
        pub fn GlobalFree(mem: Handle) -> Handle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid BMP: 14-byte file header (`BM`) + 40-byte info header +
    /// `pixels` bytes of data.
    fn bmp(pixels: &[u8]) -> Vec<u8> {
        let mut v = vec![0u8; FILE_HEADER + INFO_HEADER];
        v[0] = b'B';
        v[1] = b'M';
        v[FILE_HEADER] = INFO_HEADER as u8; // biSize = 40
        v.extend_from_slice(pixels);
        v
    }

    #[test]
    fn strips_the_file_header() {
        let image = bmp(&[1, 2, 3, 4]);
        let dib = validate(&image).expect("valid");
        // The DIB is the BMP minus its 14-byte file header.
        assert_eq!(dib.len(), image.len() - FILE_HEADER);
        assert_eq!(&dib[..2], &[INFO_HEADER as u8, 0]); // start of the info header
        assert_eq!(&dib[dib.len() - 4..], &[1, 2, 3, 4]);
    }

    #[test]
    fn rejects_empty_and_short() {
        let empty = validate(&[]).expect_err("rejected");
        assert_eq!(empty.code, AE_INVALID_IMAGE);
        assert_eq!(empty.detail, "Missing BITMAPFILEHEADER.");
        let short = validate(b"B").expect_err("rejected");
        assert_eq!(short.detail, "Missing BITMAPFILEHEADER.");
    }

    #[test]
    fn rejects_non_bmp() {
        let junk = b"notabmp\x01\x02\x03\x04\x05\x06\x07";
        let bad = validate(junk).expect_err("rejected");
        assert_eq!(bad.code, AE_INVALID_IMAGE);
        assert_eq!(bad.detail, "Invalid bitmap type.");
    }

    #[test]
    fn rejects_header_without_body() {
        // `BM` plus exactly a file header, no room for an info header: the
        // engine's bitmap build fails with 5001.
        let bad = validate(b"BM\0\0\0\0\0\0\0\0\0\0\0\0").expect_err("rejected");
        assert_eq!(bad.code, AE_ALLOCATION_FAILED);
        assert_eq!(bad.detail, "Unable to allocate memory for bitmap.");
    }
}
