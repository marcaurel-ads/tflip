//! Strings of the ACE API are in the ANSI code page; paths and file names
//! cross over to OS strings here.
//!
//! Table data is another matter. As in the original engine, the 8-bit text
//! of ADT tables (values, field and index names, expressions, keys) is bytes
//! in the client's ANSI code page, which pass through unchanged: a client
//! gets the bytes the table holds, and its own bytes are what is sought and
//! compared. The engine carries them as Windows-1252 text, which maps every
//! byte to a character of its own ([`data_in`], [`data_out`]); only where a
//! client asks for Unicode (`AdsGetFieldW`, `ADS_GET_UTF8`) are they read in
//! its ANSI code page ([`data_unicode`]). Text of the Unicode field types is
//! Unicode, converted to the ANSI code page as any other string.
//!
//! The 8-bit text of an OEM table (a Clipper or FoxPro table opened with
//! `ADS_OEM`) is bytes in the client's OEM code page, which the original
//! engine hands out converted: to the ANSI code page, best fit
//! ([`oem_to_ansi`]), or to Unicode ([`oem_to_unicode`]).
//!
//! Off Windows (host-side tests) the ANSI code page is taken to be
//! Windows-1252 and the OEM code page 437, a US Windows client's. Code page
//! 437 is converted as the original engine converts it, with tables of its
//! own; other OEM code pages as Windows converts them.

use std::ffi::{OsStr, OsString};

use tflip_storage::cp1252;

/// Table data from a client, `bytes`, as the engine carries it.
pub(crate) fn data_in(bytes: &[u8]) -> String {
    cp1252::decode(bytes)
}

/// Table data as the engine carries it, `text`, as its bytes.
pub(crate) fn data_out(text: &str) -> Vec<u8> {
    cp1252::encode(text)
}

/// Table data a client gave as Unicode (the `…W` calls, UTF-8 options), as
/// the engine carries it: in the client's ANSI code page, as the original
/// engine takes it (`'Дpfel'` finds the bytes `C4 70 66 65 6C` under code
/// page 1251).
pub(crate) fn data_from_unicode(text: &str) -> String {
    // Without best-fit characters (`A` for `Ä` under 1251), which the
    // original engine does not use: they would find what it does not.
    data_in(&substituted(text, b'?'))
}

/// Table data as the engine carries it, `text`, as Unicode: its bytes read
/// in the client's ANSI code page.
pub(crate) fn data_unicode(text: &str) -> String {
    to_string(&data_out(text))
}

/// OEM table data as the engine carries it, `text`, as ANSI bytes: read in
/// the client's OEM code page and converted, best fit (`░` is `¦`, `α` is
/// `a`), as `OemToCharBuffA` does.
pub(crate) fn oem_to_ansi(text: &str) -> Vec<u8> {
    match sys::oem_code_page() {
        437 => cp437_to_ansi(&data_out(text)),
        _ => sys::oem_to_ansi(&data_out(text)),
    }
}

/// [`oem_to_ansi`] as table data again: what the SQL engine hands on from an
/// OEM table.
pub(crate) fn oem_to_ansi_text(text: &str) -> String {
    data_in(&oem_to_ansi(text))
}

/// OEM table data as the engine carries it, `text`, as Unicode: read in the
/// client's OEM code page.
pub(crate) fn oem_to_unicode(text: &str) -> String {
    match sys::oem_code_page() {
        437 => cp437_to_unicode(&data_out(text)),
        _ => sys::oem_to_unicode(&data_out(text)),
    }
}

/// Client text in its ANSI code page, `text` as the engine carries it, as
/// OEM table data: converted to the client's OEM code page, best fit (`€` is
/// `?`, `œ` is `o`), as the original engine stores what `AdsSetString` gives
/// it in an OEM table.
pub(crate) fn ansi_to_oem(text: &str) -> String {
    let bytes = data_out(text);
    data_in(&match sys::oem_code_page() {
        437 => bytes.iter().map(|&b| ANSI_TO_OEM[usize::from(b)]).collect(),
        _ => sys::ansi_to_oem(&bytes),
    })
}

/// Client text given as Unicode, `text`, as OEM table data: in the client's
/// OEM code page, without best-fit characters; `None` if it has a character
/// the code page lacks (`AE_UNICODE_CONVERSION`), as the original engine
/// refuses what `AdsSetStringW` gives it then.
pub(crate) fn unicode_to_oem(text: &str) -> Option<String> {
    let bytes = match sys::oem_code_page() {
        437 => text
            .encode_utf16()
            .map(|unit| {
                OEM_TO_UNICODE
                    .iter()
                    .position(|&u| u == unit)
                    .map(|b| b as u8)
            })
            .collect::<Option<Vec<u8>>>()?,
        _ => sys::unicode_to_oem(text)?,
    };
    Some(data_in(&bytes))
}

/// Text read in the client's ANSI code page (as [`to_string`] does), `text`,
/// as OEM table data: converted as [`ansi_to_oem`] does.
pub(crate) fn os_text_to_oem(text: &str) -> String {
    ansi_to_oem(&data_in(&from_str_lossy(text)))
}

/// Unicode text a client gave in an expression against an OEM table, `text`,
/// as the engine carries it: as [`unicode_to_oem`], a character the code
/// page lacks becoming NUL, which no text of a table holds. The original
/// engine matches such a character to nothing, but a byte-wise text has
/// no character for that (`?` would find the `?` the table holds).
pub(crate) fn unicode_to_oem_lossy(text: &str) -> String {
    let mut utf8 = [0; 4];
    text.chars()
        .map(|c| unicode_to_oem(c.encode_utf8(&mut utf8)).unwrap_or_else(|| "\0".to_owned()))
        .collect()
}

/// Code page 437 bytes as ANSI (Windows-1252) bytes, as the original engine
/// converts them.
fn cp437_to_ansi(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(|&b| OEM_TO_ANSI[usize::from(b)]).collect()
}

/// Code page 437 bytes as Unicode, as the original engine converts them.
pub(crate) fn cp437_to_unicode(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .iter()
        .map(|&b| OEM_TO_UNICODE[usize::from(b)])
        .collect();
    String::from_utf16_lossy(&units)
}

// The original engine's conversions of code page 437, read through it from
// an OEM table holding every byte. Its own: Unicode is not what Windows'
// code page 437 gives (0x7F is U+001A, 0xE6 U+03BC).
/// Code page 437 to Windows-1252, best fit (`OemToCharBuffA`).
const OEM_TO_ANSI: [u8; 256] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F,
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F,
    0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x2B, 0x2C, 0x2D, 0x2E, 0x2F,
    0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E, 0x3F,
    0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F,
    0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F,
    0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F,
    0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x7B, 0x7C, 0x7D, 0x7E, 0x7F,
    0xC7, 0xFC, 0xE9, 0xE2, 0xE4, 0xE0, 0xE5, 0xE7, 0xEA, 0xEB, 0xE8, 0xEF, 0xEE, 0xEC, 0xC4, 0xC5,
    0xC9, 0xE6, 0xC6, 0xF4, 0xF6, 0xF2, 0xFB, 0xF9, 0xFF, 0xD6, 0xDC, 0xA2, 0xA3, 0xA5, 0x50, 0x83,
    0xE1, 0xED, 0xF3, 0xFA, 0xF1, 0xD1, 0xAA, 0xBA, 0xBF, 0xAC, 0xAC, 0xBD, 0xBC, 0xA1, 0xAB, 0xBB,
    0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0xA6, 0x2B, 0x2B, 0xA6, 0xA6, 0x2B, 0x2B, 0x2B, 0x2B, 0x2B,
    0x2B, 0x2D, 0x2D, 0x2B, 0x2D, 0x2B, 0xA6, 0xA6, 0x2B, 0x2B, 0x2D, 0x2D, 0xA6, 0x2D, 0x2B, 0x2D,
    0x2D, 0x2D, 0x2D, 0x2B, 0x2B, 0x2B, 0x2B, 0x2B, 0x2B, 0x2B, 0x2B, 0xA6, 0x5F, 0xA6, 0xA6, 0xAF,
    0x61, 0xDF, 0x47, 0x70, 0x53, 0x73, 0xB5, 0x74, 0x46, 0x54, 0x4F, 0x64, 0x38, 0x66, 0x65, 0x6E,
    0x3D, 0xB1, 0x3D, 0x3D, 0x28, 0x29, 0xF7, 0x98, 0xB0, 0xB7, 0xB7, 0x76, 0x6E, 0xB2, 0xA6, 0xA0,
];

/// Windows-1252 to code page 437, best fit, as the original engine stores
/// ANSI text in an OEM table (read through it from an OEM table given every
/// byte).
const ANSI_TO_OEM: [u8; 256] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F,
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F,
    0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x2B, 0x2C, 0x2D, 0x2E, 0x2F,
    0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E, 0x3F,
    0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F,
    0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F,
    0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F,
    0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x7B, 0x7C, 0x7D, 0x7E, 0x7F,
    0x3F, 0x3F, 0x2C, 0x9F, 0x2C, 0x2E, 0x2B, 0xD8, 0x5E, 0x25, 0x53, 0x3C, 0x4F, 0x3F, 0x5A, 0x3F,
    0x3F, 0x60, 0x27, 0x22, 0x22, 0x07, 0x2D, 0x2D, 0x7E, 0x54, 0x73, 0x3E, 0x6F, 0x3F, 0x7A, 0x59,
    0xFF, 0xAD, 0x9B, 0x9C, 0x0F, 0x9D, 0xDD, 0x15, 0x22, 0x63, 0xA6, 0xAE, 0xAA, 0x2D, 0x72, 0x5F,
    0xF8, 0xF1, 0xFD, 0x33, 0x27, 0xE6, 0x14, 0xFA, 0x2C, 0x31, 0xA7, 0xAF, 0xAC, 0xAB, 0x5F, 0xA8,
    0x41, 0x41, 0x41, 0x41, 0x8E, 0x8F, 0x92, 0x80, 0x45, 0x90, 0x45, 0x45, 0x49, 0x49, 0x49, 0x49,
    0x44, 0xA5, 0x4F, 0x4F, 0x4F, 0x4F, 0x99, 0x78, 0x4F, 0x55, 0x55, 0x55, 0x9A, 0x59, 0x5F, 0xE1,
    0x85, 0xA0, 0x83, 0x61, 0x84, 0x86, 0x91, 0x87, 0x8A, 0x82, 0x88, 0x89, 0x8D, 0xA1, 0x8C, 0x8B,
    0x64, 0xA4, 0x95, 0xA2, 0x93, 0x6F, 0x94, 0xF6, 0x6F, 0x97, 0xA3, 0x96, 0x81, 0x79, 0x5F, 0x98,
];

/// Code page 437 to Unicode (`MultiByteToWideChar` with `CP_OEMCP`).
const OEM_TO_UNICODE: [u16; 256] = [
    0x0000, 0x0001, 0x0002, 0x0003, 0x0004, 0x0005, 0x0006, 0x0007, 0x0008, 0x0009, 0x000A, 0x000B,
    0x000C, 0x000D, 0x000E, 0x000F, 0x0010, 0x0011, 0x0012, 0x0013, 0x0014, 0x0015, 0x0016, 0x0017,
    0x0018, 0x0019, 0x001C, 0x001B, 0x007F, 0x001D, 0x001E, 0x001F, 0x0020, 0x0021, 0x0022, 0x0023,
    0x0024, 0x0025, 0x0026, 0x0027, 0x0028, 0x0029, 0x002A, 0x002B, 0x002C, 0x002D, 0x002E, 0x002F,
    0x0030, 0x0031, 0x0032, 0x0033, 0x0034, 0x0035, 0x0036, 0x0037, 0x0038, 0x0039, 0x003A, 0x003B,
    0x003C, 0x003D, 0x003E, 0x003F, 0x0040, 0x0041, 0x0042, 0x0043, 0x0044, 0x0045, 0x0046, 0x0047,
    0x0048, 0x0049, 0x004A, 0x004B, 0x004C, 0x004D, 0x004E, 0x004F, 0x0050, 0x0051, 0x0052, 0x0053,
    0x0054, 0x0055, 0x0056, 0x0057, 0x0058, 0x0059, 0x005A, 0x005B, 0x005C, 0x005D, 0x005E, 0x005F,
    0x0060, 0x0061, 0x0062, 0x0063, 0x0064, 0x0065, 0x0066, 0x0067, 0x0068, 0x0069, 0x006A, 0x006B,
    0x006C, 0x006D, 0x006E, 0x006F, 0x0070, 0x0071, 0x0072, 0x0073, 0x0074, 0x0075, 0x0076, 0x0077,
    0x0078, 0x0079, 0x007A, 0x007B, 0x007C, 0x007D, 0x007E, 0x001A, 0x00C7, 0x00FC, 0x00E9, 0x00E2,
    0x00E4, 0x00E0, 0x00E5, 0x00E7, 0x00EA, 0x00EB, 0x00E8, 0x00EF, 0x00EE, 0x00EC, 0x00C4, 0x00C5,
    0x00C9, 0x00E6, 0x00C6, 0x00F4, 0x00F6, 0x00F2, 0x00FB, 0x00F9, 0x00FF, 0x00D6, 0x00DC, 0x00A2,
    0x00A3, 0x00A5, 0x20A7, 0x0192, 0x00E1, 0x00ED, 0x00F3, 0x00FA, 0x00F1, 0x00D1, 0x00AA, 0x00BA,
    0x00BF, 0x2310, 0x00AC, 0x00BD, 0x00BC, 0x00A1, 0x00AB, 0x00BB, 0x2591, 0x2592, 0x2593, 0x2502,
    0x2524, 0x2561, 0x2562, 0x2556, 0x2555, 0x2563, 0x2551, 0x2557, 0x255D, 0x255C, 0x255B, 0x2510,
    0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x255E, 0x255F, 0x255A, 0x2554, 0x2569, 0x2566,
    0x2560, 0x2550, 0x256C, 0x2567, 0x2568, 0x2564, 0x2565, 0x2559, 0x2558, 0x2552, 0x2553, 0x256B,
    0x256A, 0x2518, 0x250C, 0x2588, 0x2584, 0x258C, 0x2590, 0x2580, 0x03B1, 0x00DF, 0x0393, 0x03C0,
    0x03A3, 0x03C3, 0x03BC, 0x03C4, 0x03A6, 0x0398, 0x03A9, 0x03B4, 0x221E, 0x03C6, 0x03B5, 0x2229,
    0x2261, 0x00B1, 0x2265, 0x2264, 0x2320, 0x2321, 0x00F7, 0x2248, 0x00B0, 0x2219, 0x00B7, 0x221A,
    0x207F, 0x00B2, 0x25A0, 0x00A0,
];

/// `bytes`, in the ANSI code page, as an OS string.
pub(crate) fn to_os(bytes: &[u8]) -> OsString {
    sys::to_os(bytes)
}

/// `bytes`, in the ANSI code page, as a string.
pub(crate) fn to_string(bytes: &[u8]) -> String {
    to_os(bytes)
        .into_string()
        .unwrap_or_else(|s| s.to_string_lossy().into_owned())
}

/// `s` in the ANSI code page, or `None` if the code page cannot represent it
/// exactly (a best-fit character such as `O` for `Ω` does not count).
pub(crate) fn from_os(s: &OsStr) -> Option<Vec<u8>> {
    sys::from_os(s, false)
}

/// `s` in the ANSI code page; characters it cannot represent become the code
/// page's default character (`?`), as Windows converts them.
pub(crate) fn from_str_lossy(s: &str) -> Vec<u8> {
    sys::from_os(OsStr::new(s), true).unwrap_or_default()
}

/// The ANSI code page bytes of `s` up to its first character the code page
/// cannot represent; all of it if there is none.
pub(crate) fn from_str_prefix(s: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut utf8 = [0; 4];
    for c in s.chars() {
        match from_os(OsStr::new(c.encode_utf8(&mut utf8))) {
            Some(converted) => bytes.extend(converted),
            None => break,
        }
    }
    bytes
}

/// `s` in the ANSI code page, with 0x1A for each character the code page
/// cannot represent, as the original engine writes them when asked not to
/// fail (`ADS_DONT_CHECK_CONV_ERR`).
pub(crate) fn from_str_substituted(s: &str) -> Vec<u8> {
    substituted(s, 0x1A)
}

/// `s` in the ANSI code page, with `substitute` for each character the code
/// page cannot represent exactly.
fn substituted(s: &str, substitute: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut utf8 = [0; 4];
    for c in s.chars() {
        match from_os(OsStr::new(c.encode_utf8(&mut utf8))) {
            Some(converted) => bytes.extend(converted),
            None => bytes.push(substitute),
        }
    }
    bytes
}

#[cfg(windows)]
mod sys {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    const CP_ACP: u32 = 0;
    const CP_OEMCP: u32 = 1;
    const WC_NO_BEST_FIT_CHARS: u32 = 0x400;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MultiByteToWideChar(
            code_page: u32,
            flags: u32,
            multi_byte: *const u8,
            multi_byte_len: i32,
            wide: *mut u16,
            wide_len: i32,
        ) -> i32;
        fn WideCharToMultiByte(
            code_page: u32,
            flags: u32,
            wide: *const u16,
            wide_len: i32,
            multi_byte: *mut u8,
            multi_byte_len: i32,
            default_char: *const u8,
            used_default_char: *mut i32,
        ) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn OemToCharBuffA(source: *const u8, destination: *mut u8, length: u32) -> i32;
        fn CharToOemBuffA(source: *const u8, destination: *mut u8, length: u32) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetOEMCP() -> u32;
    }

    pub(super) fn oem_code_page() -> u32 {
        // SAFETY: no arguments.
        unsafe { GetOEMCP() }
    }

    pub(super) fn oem_to_ansi(bytes: &[u8]) -> Vec<u8> {
        let mut ansi = vec![0; bytes.len()];
        let Ok(len) = u32::try_from(bytes.len()) else {
            return ansi;
        };
        // SAFETY: both buffers hold `len` bytes.
        unsafe { OemToCharBuffA(bytes.as_ptr(), ansi.as_mut_ptr(), len) };
        ansi
    }

    pub(super) fn ansi_to_oem(bytes: &[u8]) -> Vec<u8> {
        let mut oem = vec![0; bytes.len()];
        let Ok(len) = u32::try_from(bytes.len()) else {
            return oem;
        };
        // SAFETY: both buffers hold `len` bytes.
        unsafe { CharToOemBuffA(bytes.as_ptr(), oem.as_mut_ptr(), len) };
        oem
    }

    pub(super) fn unicode_to_oem(text: &str) -> Option<Vec<u8>> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        if wide.is_empty() {
            return Some(Vec::new());
        }
        let len = i32::try_from(wide.len()).ok()?;
        let mut used_default = 0;
        // SAFETY: `wide` holds `len` units; without an output buffer the
        // call only counts.
        let n = unsafe {
            WideCharToMultiByte(
                CP_OEMCP,
                WC_NO_BEST_FIT_CHARS,
                wide.as_ptr(),
                len,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        let mut bytes = vec![0u8; n.max(0) as usize];
        // SAFETY: `wide` holds `len` units and `bytes` `n` bytes.
        let n = unsafe {
            WideCharToMultiByte(
                CP_OEMCP,
                WC_NO_BEST_FIT_CHARS,
                wide.as_ptr(),
                len,
                bytes.as_mut_ptr(),
                n,
                std::ptr::null(),
                &mut used_default,
            )
        };
        bytes.truncate(n.max(0) as usize);
        (used_default == 0).then_some(bytes)
    }

    pub(super) fn oem_to_unicode(bytes: &[u8]) -> String {
        let Ok(len) = i32::try_from(bytes.len()) else {
            return String::new();
        };
        if len == 0 {
            return String::new();
        }
        String::from_utf16_lossy(&to_wide(CP_OEMCP, bytes, len))
    }

    pub(super) fn to_os(bytes: &[u8]) -> OsString {
        let Ok(len) = i32::try_from(bytes.len()) else {
            return OsString::new();
        };
        if len == 0 {
            return OsString::new();
        }
        OsString::from_wide(&to_wide(CP_ACP, bytes, len))
    }

    /// `bytes`, `len` of them, in `code_page`, as UTF-16.
    fn to_wide(code_page: u32, bytes: &[u8], len: i32) -> Vec<u16> {
        // SAFETY: `bytes` holds `len` bytes; without an output buffer the
        // call only counts.
        let n = unsafe {
            MultiByteToWideChar(code_page, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0)
        };
        let mut wide = vec![0u16; n.max(0) as usize];
        // SAFETY: `bytes` holds `len` bytes and `wide` `n` units.
        let n =
            unsafe { MultiByteToWideChar(code_page, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), n) };
        wide.truncate(n.max(0) as usize);
        wide
    }

    pub(super) fn from_os(s: &OsStr, lossy: bool) -> Option<Vec<u8>> {
        let wide: Vec<u16> = s.encode_wide().collect();
        let len = i32::try_from(wide.len()).ok()?;
        if len == 0 {
            return Some(Vec::new());
        }
        let null = std::ptr::null();
        let mut used_default = 0;
        // Exactly: no best-fit characters (`O` for `Ω`), as the original
        // engine converts.
        let flags = if lossy { 0 } else { WC_NO_BEST_FIT_CHARS };
        // SAFETY: `wide` holds `len` units; without an output buffer the
        // call only counts.
        let n = unsafe {
            WideCharToMultiByte(
                CP_ACP,
                flags,
                wide.as_ptr(),
                len,
                std::ptr::null_mut(),
                0,
                null,
                &mut used_default,
            )
        };
        let mut bytes = vec![0u8; n.max(0) as usize];
        // SAFETY: `wide` holds `len` units and `bytes` `n` bytes.
        let n = unsafe {
            WideCharToMultiByte(
                CP_ACP,
                flags,
                wide.as_ptr(),
                len,
                bytes.as_mut_ptr(),
                n,
                null,
                &mut used_default,
            )
        };
        if n <= 0 || used_default != 0 && !lossy {
            return None;
        }
        bytes.truncate(n as usize);
        Some(bytes)
    }
}

#[cfg(not(windows))]
mod sys {
    use std::ffi::{OsStr, OsString};

    use tflip_storage::cp1252;

    pub(super) fn to_os(bytes: &[u8]) -> OsString {
        cp1252::decode(bytes).into()
    }

    pub(super) fn from_os(s: &OsStr, lossy: bool) -> Option<Vec<u8>> {
        let s = s.to_str()?;
        let bytes = cp1252::encode(s);
        (lossy || cp1252::decode(&bytes) == s).then_some(bytes)
    }

    pub(super) fn oem_code_page() -> u32 {
        437
    }

    pub(super) fn oem_to_ansi(bytes: &[u8]) -> Vec<u8> {
        super::cp437_to_ansi(bytes)
    }

    pub(super) fn oem_to_unicode(bytes: &[u8]) -> String {
        super::cp437_to_unicode(bytes)
    }

    // Off Windows the OEM code page is 437, which has tables of its own.
    pub(super) fn ansi_to_oem(bytes: &[u8]) -> Vec<u8> {
        bytes.to_vec()
    }

    pub(super) fn unicode_to_oem(_: &str) -> Option<Vec<u8>> {
        None
    }
}
