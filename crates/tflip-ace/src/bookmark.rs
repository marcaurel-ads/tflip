//! Bookmarks: where a table was, kept to go back to or to put in order.
//!
//! As in the original engine (`fixtures/bookmarks/bookmarks.json`, from
//! `tools/ace/bookmarks/`), a bookmark taken through a table or cursor is 8
//! bytes: the record number and 0. Through an index order it is the record
//! number, the length of the order's keys, the order's handle and the key
//! of the record, so `12 + key length` bytes. A comparison of two of them
//! finds the order by the handle in them.

use tflip_engine::handle::HandleKind;

use crate::backend::api;
use crate::last_error::fail_detail;
use crate::{
    AE_INSUFFICIENT_BUFFER, AE_INVALID_BOOKMARK, AE_INVALID_HANDLE, AE_INVALID_POINTER,
    AE_INVALID_TABLE_HANDLE, AE_SUCCESS, AdsHandle, Signed32, Unsigned8, Unsigned32, error_code,
};

/// What a bookmark is taken through.
enum Through {
    /// A table or cursor.
    Table,
    /// An index order.
    Order,
}

/// Checks that `handle` is a handle at all, the first thing the original
/// engine does with a bookmark call's handle.
fn known(handle: AdsHandle) -> Result<(), Unsigned32> {
    match api(handle).handle_kind(handle) {
        Ok(None) => {
            // A handle of 0 is "not readable", any other value "not recognized".
            let detail = if handle == 0 {
                "The given handle is not readable."
            } else {
                crate::last_error::NOT_A_HANDLE
            };
            Err(fail_detail(AE_INVALID_HANDLE, detail))
        }
        Err(error) => Err(error_code(error)),
        Ok(Some(_)) => Ok(()),
    }
}

/// What kind of handle a bookmark is taken through; the original engine
/// fails any other kind after it has checked the pointers.
fn through(handle: AdsHandle) -> Result<Through, Unsigned32> {
    match api(handle).handle_kind(handle) {
        Ok(Some(HandleKind::Table | HandleKind::Cursor)) => Ok(Through::Table),
        Ok(Some(HandleKind::Index)) => Ok(Through::Order),
        Err(error) => Err(error_code(error)),
        Ok(_) => Err(fail_detail(
            AE_INVALID_HANDLE,
            "Must be a table, cursor or index order handle.",
        )),
    }
}

/// The bytes of a bookmark of the current record. Off a record the record
/// number is the one `AdsGetRecordNum` gives (0 at BOF, one past the last
/// record at EOF), and the key an order's first at BOF and its last at EOF.
fn bookmark(handle: AdsHandle, through: &Through) -> Result<Vec<u8>, Unsigned32> {
    let backend = api(handle);
    let table = match through {
        Through::Table => handle,
        Through::Order => backend.index_info(handle).map_err(error_code)?.table,
    };
    let record = backend.record_number(table, false).map_err(error_code)?;
    let mut bytes = record.to_le_bytes().to_vec();
    match through {
        Through::Table => bytes.extend_from_slice(&[0; 4]),
        Through::Order => {
            let key = backend.bookmark_key(handle).map_err(error_code)?;
            bytes.extend_from_slice(&(key.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&(handle as u32).to_le_bytes());
            bytes.extend_from_slice(&key);
        }
    }
    Ok(bytes)
}

/// `AdsGetBookmarkLength`: how many bytes a bookmark takes, the buffer size
/// `AdsGetBookmark60` needs: 8 through a table, and 12 and the order's key
/// length through an index order.
///
/// # Safety
///
/// `length` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetBookmarkLength(
    table: AdsHandle,
    length: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetBookmarkLength");
    if let Err(code) = known(table) {
        return code;
    }
    if length.is_null() {
        return fail_detail(AE_INVALID_POINTER, "Parameter 2");
    }
    let value = match through(table) {
        Ok(Through::Table) => 8,
        Ok(Through::Order) => match api(table).index_info(table) {
            Ok(info) => 12 + Unsigned32::from(info.key_length),
            Err(error) => return error_code(error),
        },
        Err(code) => return code,
    };
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *length = value };
    AE_SUCCESS
}

/// Checks the handle of the handle-sized bookmark calls: a table or cursor.
/// The original engine fails an index order with `AE_INVALID_TABLE_HANDLE`
/// and the text of that, and any other handle, one that is none too, with
/// that code and the text of `AE_INVALID_HANDLE`.
fn handle_sized(handle: AdsHandle) -> Result<(), Unsigned32> {
    match api(handle).handle_kind(handle) {
        Ok(Some(HandleKind::Table | HandleKind::Cursor)) => Ok(()),
        Ok(Some(HandleKind::Index)) => Err(fail_detail(
            AE_INVALID_TABLE_HANDLE,
            "The handle must be to a table.",
        )),
        Err(error) => Err(error_code(error)),
        Ok(_) => {
            fail_detail(
                AE_INVALID_HANDLE,
                "Must be a table, cursor or index order handle.",
            );
            Err(AE_INVALID_TABLE_HANDLE)
        }
    }
}

/// `AdsGetBookmark`: a bookmark of the current record as a handle-sized
/// value for `AdsGotoBookmark`: its record number, as in the original engine
/// for free tables. Like `AdsGetRecordNum`, it is 0 at BOF and one past the
/// last record at EOF. Only a table or cursor takes it.
///
/// # Safety
///
/// `bookmark` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetBookmark(
    table: AdsHandle,
    bookmark: *mut AdsHandle,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetBookmark");
    let value = handle_sized(table).and_then(|()| {
        api(table)
            .record_number(table, false)
            .map(|record| record as AdsHandle)
            .map_err(error_code)
    });
    // SAFETY: `bookmark` is null or writable, as # Safety says.
    unsafe { crate::field::write(value, bookmark) }
}

/// Moves `table` to the record a bookmark names, as `AdsGotoRecord` does
/// (a deleted record of an ADT table is refused, and the table is then at
/// EOF); a record number that is not one of the table's slots, 0 included,
/// is `AE_INVALID_BOOKMARK` and leaves the position alone.
fn goto_bookmark(table: AdsHandle, record: u32) -> Unsigned32 {
    let slots = api(table)
        .table_info(table)
        .map(|info| info.structure.header.record_slots);
    match slots {
        Err(error) => error_code(error),
        Ok(slots) if (1..=slots).contains(&record) => crate::AdsGotoRecord(table, record),
        Ok(_) => fail_detail(
            AE_INVALID_BOOKMARK,
            "Bookmark was not on a valid record for this table.",
        ),
    }
}

/// `AdsGotoBookmark`: moves to the record of a bookmark from
/// `AdsGetBookmark`. Only a table or cursor takes it.
#[unsafe(no_mangle)]
pub extern "system" fn AdsGotoBookmark(table: AdsHandle, bookmark: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGotoBookmark");
    if let Err(code) = handle_sized(table) {
        return code;
    }
    // A value that is no record number cannot be one of the table's.
    goto_bookmark(table, u32::try_from(bookmark).unwrap_or(0))
}

/// `AdsGetBookmark60`: a bookmark of the current record in `buf` of `*len`
/// bytes (see `AdsGetBookmarkLength`); `*len` is set to its length. A buffer
/// that is too small fails with `AE_INSUFFICIENT_BUFFER` and is not written.
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetBookmark60(
    table: AdsHandle,
    buf: *mut Unsigned8,
    len: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetBookmark60");
    if let Err(code) = known(table) {
        return code;
    }
    if buf.is_null() {
        return fail_detail(AE_INVALID_POINTER, "Parameter 2");
    }
    if len.is_null() {
        return fail_detail(AE_INVALID_POINTER, "Parameter 3");
    }
    let bytes = match through(table).and_then(|through| bookmark(table, &through)) {
        Ok(bytes) => bytes,
        Err(code) => return code,
    };
    // SAFETY: neither is null, so `len` is readable and writable and `buf`
    // holds `*len` bytes, at least as many as are copied.
    unsafe {
        let room = *len;
        *len = bytes.len() as Unsigned32;
        if room < *len {
            return fail_detail(
                AE_INSUFFICIENT_BUFFER,
                "Bookmark buffer was not big enough.",
            );
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
    }
    AE_SUCCESS
}

/// The word of a bookmark at `offset`.
///
/// # Safety
///
/// `bookmark` must hold four bytes at `offset`.
unsafe fn word(bookmark: *const Unsigned8, offset: usize) -> u32 {
    // SAFETY: as the caller promises.
    unsafe { bookmark.add(offset).cast::<u32>().read_unaligned() }
}

/// `AdsGotoBookmark60`: moves to the record of a bookmark from
/// `AdsGetBookmark60`, by its record number. Only a table or cursor moves:
/// through an index order, or a connection, the original engine does
/// nothing and succeeds.
///
/// # Safety
///
/// `bookmark` must be null or point to a bookmark of `AdsGetBookmarkLength`
/// bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGotoBookmark60(
    table: AdsHandle,
    bookmark: *mut Unsigned8,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGotoBookmark60");
    if let Err(code) = known(table) {
        return code;
    }
    if bookmark.is_null() {
        return fail_detail(AE_INVALID_BOOKMARK, "Parameter 2");
    }
    if !matches!(through(table), Ok(Through::Table)) {
        return AE_SUCCESS;
    }
    // SAFETY: a bookmark holds its record number first, as # Safety says.
    goto_bookmark(table, unsafe { word(bookmark, 0) })
}

/// `AdsCompareBookmarks`: orders two bookmarks from `AdsGetBookmark60`:
/// `*result` is negative, 0 or positive as the first comes before, is, or
/// comes after the second.
///
/// As in the original engine, two bookmarks taken through the same index
/// order are ordered by their keys (as many bytes as they say they have, and
/// they must say the same number: `AE_INVALID_BOOKMARK` otherwise) and then
/// by their records, the other way round for a descending order. Any others,
/// those of a table, of different orders, or of a key length of 0, are
/// ordered by their records alone.
///
/// # Safety
///
/// `first` and `second` must each be null or point to a bookmark of
/// `AdsGetBookmarkLength` bytes; `result` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCompareBookmarks(
    first: *mut Unsigned8,
    second: *mut Unsigned8,
    result: *mut Signed32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsCompareBookmarks");
    for (position, bookmark) in [first, second].into_iter().enumerate() {
        if bookmark.is_null() {
            return fail_detail(AE_INVALID_POINTER, &format!("Parameter {}", position + 1));
        }
    }
    if result.is_null() {
        return fail_detail(AE_INVALID_POINTER, "Parameter 3");
    }
    // SAFETY: bookmarks hold a record number and a key length first, and
    // with a key length, the order's handle and the key, as # Safety says.
    let order = unsafe {
        let (records, lengths) = (
            (word(first, 0), word(second, 0)),
            (word(first, 4), word(second, 4)),
        );
        let mut order = records.0.cmp(&records.1);
        if lengths.0 != 0 && lengths.1 != 0 && word(first, 8) == word(second, 8) {
            if lengths.0 != lengths.1 {
                // The original engine has set the result to 0 by then.
                *result = 0;
                return fail_detail(AE_INVALID_BOOKMARK, "Bookmark key sizes are not the same.");
            }
            let handle = word(first, 8) as AdsHandle;
            let descending = matches!(api(handle).handle_kind(handle), Ok(Some(HandleKind::Index)))
                && api(handle)
                    .index_info(handle)
                    .is_ok_and(|info| info.descending);
            let keys = (
                std::slice::from_raw_parts(first.add(12), lengths.0 as usize),
                std::slice::from_raw_parts(second.add(12), lengths.1 as usize),
            );
            order = keys.0.cmp(keys.1).then(order);
            if descending {
                order = order.reverse();
            }
        }
        order
    };
    // SAFETY: not null, so writable, as # Safety says.
    unsafe { *result = order as Signed32 };
    AE_SUCCESS
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;

    use serde_json::{Map, Value as Json, json};

    use super::*;
    use crate::fixtures;

    fn last_text() -> String {
        let (mut code, mut buf, mut len) = (0, [0u8; 1200], 1200);
        unsafe { crate::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
        String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned()
    }

    /// A bookmark's bytes as the fixture writes them: hex, with the handle
    /// of an index order at offset 8 as `{hN}`.
    fn hex(bytes: &[u8], orders: &[AdsHandle]) -> String {
        let mut out = String::new();
        let mut i = 0;
        while i < bytes.len() {
            if i == 8 && i + 4 <= bytes.len() {
                let word = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
                if let Some(n) = orders.iter().position(|&order| order as u32 == word) {
                    out += &format!("{{h{n}}}");
                    i += 4;
                    continue;
                }
            }
            out += &format!("{:02x}", bytes[i]);
            i += 1;
        }
        out
    }

    fn unhex(text: &str, orders: &[AdsHandle], out: &mut [u8]) {
        let mut rest = text;
        let mut n = 0;
        while !rest.is_empty() {
            if let Some(tail) = rest.strip_prefix("{h") {
                let (digit, tail) = tail.split_once('}').unwrap();
                let word = orders[digit.parse::<usize>().unwrap()] as u32;
                out[n..n + 4].copy_from_slice(&word.to_le_bytes());
                n += 4;
                rest = tail;
            } else {
                out[n] = u8::from_str_radix(&rest[..2], 16).unwrap();
                n += 1;
                rest = &rest[2..];
            }
        }
    }

    /// A bookmark in a register of the script: 64 bytes and its length.
    type Register = ([u8; 64], u32);

    /// Replays `fixtures/bookmarks/bookmarks.json`, what the original engine
    /// does with bookmarks taken through a table and through its index
    /// orders (`tools/ace/bookmarks/`, whose `ops.txt` is the script).
    #[test]
    fn bookmarks_match_the_original_engine() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../");
        let rows: Vec<Json> = serde_json::from_slice(
            &std::fs::read(root.join("fixtures/bookmarks/bookmarks.json")).unwrap(),
        )
        .unwrap();
        let ops = std::fs::read_to_string(root.join("tools/ace/bookmarks/ops.txt")).unwrap();
        let ops: Vec<&str> = ops
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect();
        assert_eq!(ops.len(), rows.len());
        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        let mut orders = [0 as AdsHandle; 8];
        let mut count = 8;
        assert_eq!(
            unsafe { crate::AdsGetAllIndexes(table, orders.as_mut_ptr(), &mut count) },
            AE_SUCCESS
        );
        let orders = &orders[..usize::from(count)];
        let handle = |name: &str| match name {
            "t" => table,
            "bad" => 0x12345,
            "zero" => 0,
            "conn" => connect,
            number => orders[number.parse::<usize>().unwrap()],
        };
        let recno = || {
            let mut record = 0;
            unsafe { crate::AdsGetRecordNum(table, 2, &mut record) };
            record
        };
        let mut registers: HashMap<&str, Register> = HashMap::new();
        let mut differ = Vec::new();
        for (line, want) in ops.iter().zip(&rows) {
            let words: Vec<&str> = line.split(' ').collect();
            let mut got = Map::new();
            let mut with_recno = false;
            let rc = match words[0] {
                "move" => {
                    let h = handle(words[1]);
                    let rc = match words[2] {
                        "top" => crate::AdsGotoTop(h),
                        "bottom" => crate::AdsGotoBottom(h),
                        "bof" => {
                            crate::AdsGotoTop(h);
                            crate::AdsSkip(h, -1)
                        }
                        "eof" => {
                            crate::AdsGotoBottom(h);
                            crate::AdsSkip(h, 1)
                        }
                        _ => crate::AdsGotoRecord(table, words[3].parse().unwrap()),
                    };
                    with_recno = true;
                    rc
                }
                "len" | "lennull" => {
                    let mut len = 99;
                    let null = words[0] == "lennull";
                    let pointer = if null { std::ptr::null_mut() } else { &mut len };
                    let rc = unsafe { AdsGetBookmarkLength(handle(words[1]), pointer) };
                    if !null {
                        got.insert("len".into(), json!(len));
                    }
                    rc
                }
                "get" => {
                    let register = registers
                        .entry(words.get(2).copied().unwrap_or("_"))
                        .or_insert(([0xEE; 64], 0));
                    register.0 = [0xEE; 64];
                    let mut len = words.get(3).map_or(64, |size| size.parse().unwrap());
                    let rc = unsafe {
                        AdsGetBookmark60(handle(words[1]), register.0.as_mut_ptr(), &mut len)
                    };
                    register.1 = if rc == 0 { len } else { 0 };
                    got.insert("len".into(), json!(len));
                    let shown = if rc != 0 { 8 } else { len.min(64) as usize };
                    got.insert("bytes".into(), json!(hex(&register.0[..shown], orders)));
                    rc
                }
                "getnull" => {
                    let (mut buf, mut len) = ([0u8; 64], 64);
                    let (b, l): (*mut u8, *mut u32) = match words[2] {
                        "buf" => (std::ptr::null_mut(), &mut len),
                        _ => (buf.as_mut_ptr(), std::ptr::null_mut()),
                    };
                    unsafe { AdsGetBookmark60(handle(words[1]), b, l) }
                }
                "get1" => {
                    let mut value = 0x77;
                    let rc = unsafe { AdsGetBookmark(handle(words[1]), &mut value) };
                    got.insert("value".into(), json!(value));
                    rc
                }
                "copy" => {
                    let source = *registers.entry(words[2]).or_insert(([0xEE; 64], 0));
                    registers.insert(words[1], source);
                    0
                }
                "patch" => {
                    let register = registers.entry(words[1]).or_insert(([0xEE; 64], 0));
                    let offset = words[2].parse::<usize>().unwrap();
                    unhex(words[3], orders, &mut register.0[offset..]);
                    let shown = &register.0[..register.1 as usize];
                    got.insert("bytes".into(), json!(hex(shown, orders)));
                    0
                }
                "cmp" => {
                    let mut result = 99;
                    let mut a = registers.entry(words[1]).or_insert(([0xEE; 64], 0)).0;
                    let mut b = registers.entry(words[2]).or_insert(([0xEE; 64], 0)).0;
                    let rc =
                        unsafe { AdsCompareBookmarks(a.as_mut_ptr(), b.as_mut_ptr(), &mut result) };
                    got.insert("result".into(), json!(result));
                    rc
                }
                "cmpnull" => {
                    let mut result = 99;
                    let mut p = registers.entry("p").or_insert(([0xEE; 64], 0)).0;
                    let (mut x, mut y, mut r) =
                        (p.as_mut_ptr(), p.as_mut_ptr(), &mut result as *mut _);
                    match words[1] {
                        "a" => x = std::ptr::null_mut(),
                        "b" => y = std::ptr::null_mut(),
                        _ => r = std::ptr::null_mut(),
                    }
                    let rc = unsafe { AdsCompareBookmarks(x, y, r) };
                    got.insert("result".into(), json!(result));
                    rc
                }
                "goto" => {
                    let mut bytes = registers.entry(words[2]).or_insert(([0xEE; 64], 0)).0;
                    let rc = unsafe { AdsGotoBookmark60(handle(words[1]), bytes.as_mut_ptr()) };
                    with_recno = true;
                    rc
                }
                "gotonull" => {
                    let rc = unsafe { AdsGotoBookmark60(handle(words[1]), std::ptr::null_mut()) };
                    with_recno = true;
                    rc
                }
                "goto1" => {
                    let rc = AdsGotoBookmark(handle(words[1]), words[2].parse().unwrap());
                    with_recno = true;
                    rc
                }
                other => panic!("{other}"),
            };
            got.insert("rc".into(), json!(rc));
            if rc != 0 {
                got.insert("error".into(), json!(last_text()));
            }
            if with_recno {
                got.insert("recno".into(), json!(recno()));
            }
            let mut want = want.as_object().unwrap().clone();
            assert_eq!(want.remove("cmd").unwrap(), *line);
            let mut keys: Vec<&String> = want.keys().chain(got.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                if want.get(key) != got.get(key) {
                    differ.push(format!(
                        "{line}: {key}: got {:?} want {:?}",
                        got.get(key),
                        want.get(key)
                    ));
                }
            }
        }
        crate::AdsDisconnect(connect);
        assert!(
            differ.is_empty(),
            "{} differ:\n{}",
            differ.len(),
            differ.join("\n")
        );
    }

    crate::fixtures::remote_twins!(bookmarks_match_the_original_engine);
}
