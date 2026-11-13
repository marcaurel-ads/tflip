//! The engine's conversion of the four double-byte code pages — 932
//! (Shift-JIS), 936 (GBK), 949 (UHC) and 950 (Big5) — for
//! [`AdsSetStringFromCodePage`](crate::setters::AdsSetStringFromCodePage).
//!
//! The engine converts through its bundled ICU (`aicu32.dll`), not the host's
//! NLS, so these are the engine's own tables, probed byte-pair by byte-pair
//! through the real DLL under Wine (`tools/ace/codepage`,
//! `fixtures/codepage/dbcs.json`) and baked into [`TABLES`] by
//! `tools/ace/codepage/gen_dbcs.py`.
//!
//! The engine reads a byte string left to right. A **lead byte** (bit set in
//! the page's lead bitmap) takes the following byte as its trail and the pair
//! makes one character; a lead byte at the end of the string, or a pair the
//! page does not map, becomes U+FFFD. Every other byte converts on its own
//! (ASCII, the half-width katakana of 932, or U+FFFD where undefined). A lead
//! byte always consumes two bytes and yields exactly one unit, so an invalid
//! trail does not re-align the stream — matching what the probe recorded.

use std::sync::OnceLock;

/// The per-page tables, generated from the probed golden by
/// `tools/ace/codepage/gen_dbcs.py` (see the module docs for the layout).
static TABLES: &[u8] = include_bytes!("dbcs_tables.bin");

/// The Unicode replacement character, which the engine emits for an undefined
/// byte, an unmapped pair, or a lead byte with no trail.
const REPLACEMENT: u16 = 0xFFFD;

/// One code page's conversion tables.
struct Page {
    code_page: u32,
    /// The unit each byte converts to on its own (U+FFFD for a lead or
    /// undefined byte; U+0000 for a NUL).
    single: [u16; 256],
    /// Whether each byte is a lead byte (consumes the following byte).
    is_lead: [bool; 256],
    /// For each lead byte, the index of its row in [`Self::rows`], or
    /// `u16::MAX` when the byte is not a lead or maps every trail to U+FFFD.
    row_base: [u16; 256],
    /// One row of 256 trail units per mapped lead byte.
    rows: Vec<[u16; 256]>,
}

/// Reads the next little-endian `u16` at `*off`, advancing it.
fn u16_at(bytes: &[u8], off: &mut usize) -> u16 {
    let v = u16::from_le_bytes([bytes[*off], bytes[*off + 1]]);
    *off += 2;
    v
}

/// Parses [`TABLES`] once.
fn pages() -> &'static [Page] {
    static PAGES: OnceLock<Vec<Page>> = OnceLock::new();
    PAGES.get_or_init(|| {
        let mut off = 0;
        let npages = u16_at(TABLES, &mut off) as usize;
        let mut pages = Vec::with_capacity(npages);
        for _ in 0..npages {
            let code_page = u32::from_le_bytes(TABLES[off..off + 4].try_into().unwrap());
            off += 4;
            let mut single = [0u16; 256];
            for unit in &mut single {
                *unit = u16_at(TABLES, &mut off);
            }
            let bitmap = &TABLES[off..off + 32];
            off += 32;
            let mut is_lead = [false; 256];
            for (b, flag) in is_lead.iter_mut().enumerate() {
                *flag = bitmap[b >> 3] & (1 << (b & 7)) != 0;
            }
            let mut row_base = [0u16; 256];
            for base in &mut row_base {
                *base = u16_at(TABLES, &mut off);
            }
            let nrows = u16_at(TABLES, &mut off) as usize;
            let mut rows = Vec::with_capacity(nrows);
            for _ in 0..nrows {
                let mut row = [0u16; 256];
                for unit in &mut row {
                    *unit = u16_at(TABLES, &mut off);
                }
                rows.push(row);
            }
            pages.push(Page {
                code_page,
                single,
                is_lead,
                row_base,
                rows,
            });
        }
        pages
    })
}

/// The UTF-16 units `bytes` convert to in double-byte code page `code_page`, as
/// the engine's `AdsSetStringFromCodePage` reads them, or `None` when
/// `code_page` is not one of the four double-byte pages.
pub(crate) fn decode(code_page: u32, bytes: &[u8]) -> Option<Vec<u16>> {
    let page = pages().iter().find(|p| p.code_page == code_page)?;
    let mut units = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = usize::from(bytes[i]);
        if page.is_lead[b] {
            if let Some(&trail) = bytes.get(i + 1) {
                units.push(match page.row_base[b] {
                    u16::MAX => REPLACEMENT,
                    base => page.rows[usize::from(base)][usize::from(trail)],
                });
                i += 2;
            } else {
                units.push(REPLACEMENT); // a lead byte with no trail
                i += 1;
            }
        } else {
            units.push(page.single[b]);
            i += 1;
        }
    }
    Some(units)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Every double-byte page converts each single byte, each lead/trail pair
    /// and each lead byte at the end of the string exactly as the original
    /// engine's `AdsSetStringFromCodePage` did (`fixtures/codepage/dbcs.json`,
    /// probed with `tools/ace/codepage`).
    #[test]
    fn decode_matches_the_engine() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/codepage/dbcs.json");
        let golden: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let pages = golden["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 4);
        for page in pages {
            let cp = page["code_page"].as_u64().unwrap() as u32;
            let single = page["single_u"].as_array().unwrap();

            // A lone NUL is U+0000 (the golden records it as it reads back from
            // a field, a space); every other single byte matches the golden,
            // lead bytes included (a lead with no trail is U+FFFD).
            assert_eq!(decode(cp, &[0]).unwrap(), vec![0], "cp {cp} byte 0");
            for (byte, unit) in single.iter().enumerate().skip(1) {
                let unit = unit.as_u64().unwrap() as u16;
                assert_eq!(
                    decode(cp, &[byte as u8]).unwrap(),
                    vec![unit],
                    "cp {cp} byte {byte:#04x}"
                );
            }

            for lead in page["leads"].as_array().unwrap() {
                let l = lead["lead"].as_u64().unwrap() as u8;
                let us = lead["u"].as_array().unwrap();
                for (trail, unit) in us.iter().enumerate() {
                    let unit = unit.as_u64().unwrap() as u16;
                    assert_eq!(
                        decode(cp, &[l, trail as u8]).unwrap(),
                        vec![unit],
                        "cp {cp} pair {l:#04x},{trail:#04x}"
                    );
                }
            }
        }
    }

    /// A mixed string converts character by character, a lead byte consuming
    /// its trail and single bytes standing alone.
    #[test]
    fn decode_mixed_string() {
        // 932: full-width A (0x82A0 -> U+3042), ASCII 'A', half-width katakana
        // 'o' (single byte 0xA1 -> U+FF61), and a lead byte with no trail.
        assert_eq!(
            decode(932, &[0x82, 0xA0, 0x41, 0xA1, 0x81]).unwrap(),
            vec![0x3042, 0x0041, 0xFF61, 0xFFFD],
        );
        assert_eq!(decode(932, &[]).unwrap(), Vec::<u16>::new());
    }

    /// Only the four double-byte pages are known here.
    #[test]
    fn other_pages_are_not_double_byte() {
        for cp in [0, 437, 1252, 1200, 65001] {
            assert!(decode(cp, b"x").is_none(), "{cp}");
        }
    }
}
