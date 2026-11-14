//! Positions in index orders: scopes, key numbers and counts, relative key
//! positions, skipping duplicate keys, and the direction of an order.

use crate::backend::api;
use crate::ffi::slice;
use crate::field::write_at;
use crate::keys::read_key;
use crate::last_error::{fail, fail_detail};
use crate::{
    ADS_BOTTOM, ADS_IGNOREFILTERS, ADS_RESPECTSCOPES, ADS_TOP, AE_INVALID_FILTER_OPTION,
    AE_INVALID_OPTION, AE_INVALID_POINTER, AE_NO_SCOPE, AE_SUCCESS, AdsHandle, Signed32, Unsigned8,
    Unsigned16, Unsigned32, code, error_code, write_cut,
};

/// Whether `AdsGetKeyNum` and `AdsGetKeyCount` respect the order's scope
/// and the table's filters (deleted records hidden among them) with
/// `filter_option`, as the original engine does (`fixtures/seek/
/// deleted.json`): `ADS_RESPECTFILTERS` does, `ADS_IGNOREFILTERS` does not,
/// and `ADS_RESPECTSCOPES` does while the order has a scope; without one it
/// counts every key, as `ADS_IGNOREFILTERS` does.
fn respects(index: AdsHandle, filter_option: Unsigned16) -> bool {
    match filter_option {
        ADS_IGNOREFILTERS => false,
        ADS_RESPECTSCOPES => {
            let backend = api(index);
            backend.scope_key(index, false).is_ok() || backend.scope_key(index, true).is_ok()
        }
        _ => true,
    }
}

/// `AdsGetKeyNum`: the current record's place in an index order, 1-based:
/// 0 at BOF and one past the last key at EOF, as in the original engine. On a
/// record the order has no key for it fails with `AE_NOT_FOUND`, leaving
/// `*key` alone. Where the scope and the filters are respected ([`respects`]),
/// the place counts from the order's scope and over visible records only
/// (see [`tflip_engine::table::Table::key_number_in`]).
///
/// # Safety
///
/// `key` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetKeyNum(
    index: AdsHandle,
    filter_option: Unsigned16,
    key: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetKeyNum");
    let scoped = respects(index, filter_option);
    let number = api(index).key_number(index, scoped).map_err(error_code);
    // The pointer is checked before the option, unlike `AdsGetKeyCount`'s.
    if number.is_ok() && key.is_null() {
        return crate::last_error::null_param(3);
    }
    if number.is_ok() && !(1..=3).contains(&filter_option) {
        return fail_detail(AE_INVALID_FILTER_OPTION, "Parameter 2");
    }
    // SAFETY: `key` is null or writable, as # Safety says.
    unsafe { write_at(number, key, 3) }
}

/// `AdsGetKeyCount`: the number of keys in an index order: one per record it
/// holds (fewer than the table's records for a conditional or custom order),
/// but the bits of its bitmaps for a binary order, as in the original engine.
/// Where the scope and the filters are respected ([`respects`]), only the
/// keys in the order's scope count, of records neither the filters hide
/// nor deleted while deleted records are hidden.
///
/// # Safety
///
/// `count` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetKeyCount(
    index: AdsHandle,
    filter_option: Unsigned16,
    count: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetKeyCount");
    let scoped = respects(index, filter_option);
    let keys = api(index).key_count(index, scoped).map_err(error_code);
    if keys.is_ok() && !(1..=3).contains(&filter_option) {
        return fail_detail(AE_INVALID_FILTER_OPTION, "Parameter 2");
    }
    // SAFETY: `count` is null or writable, as # Safety says.
    unsafe { write_at(keys, count, 3) }
}

/// `AdsSkipUnique`: moves `records` distinct keys forward in an index
/// order, or back if negative: forward to the first record of each next
/// key, back to the last record of each previous one, as in the original
/// engine.
#[unsafe(no_mangle)]
pub extern "system" fn AdsSkipUnique(index: AdsHandle, records: Signed32) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSkipUnique");
    let backend = api(index);
    if let Err(error) = backend.index_info(index) {
        return error_code(error);
    }
    code(backend.skip_unique(index, i64::from(records)))
}

/// `AdsSetIndexDirection`: any value but 0 turns an index order round, so
/// that it is walked the other way from how it is walked now (1 and 2
/// alike), and `AdsIsIndexDescending` says so; 0 leaves it as it is. Key
/// numbers and relative positions follow. An NTX order is refused with
/// `AE_INVALID_INDEX_TYPE` whatever the value, as in the original engine
/// (`fixtures/seek/direction.json`).
#[unsafe(no_mangle)]
pub extern "system" fn AdsSetIndexDirection(index: AdsHandle, flip: Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetIndexDirection");
    code(api(index).set_index_direction(index, flip != 0))
}

/// `AdsGetRelKeyPos`: where the current key is in an index order, from 0
/// to 1 (see [`tflip_engine::table::Table::relative_position_in`]). At BOF
/// or EOF it fails with `AE_NO_CURRENT_RECORD`.
///
/// # Safety
///
/// `position` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetRelKeyPos(index: AdsHandle, position: *mut f64) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetRelKeyPos");
    // SAFETY: `position` is null or writable, as # Safety says.
    let position = unsafe { position.as_mut() };
    if let Err(error) = api(index).index_info(index) {
        return error_code(error);
    }
    let Some(position) = position else {
        // The original engine names the argument after it.
        return crate::last_error::null_param(3);
    };
    match api(index).relative_position(index) {
        Ok(at) => {
            *position = at;
            AE_SUCCESS
        }
        Err(error) => error_code(error),
    }
}

/// `AdsSetRelKeyPos`: moves to the key at `position` (0 to 1) of an index
/// order (see [`tflip_engine::table::Table::set_relative_position_in`]).
/// Positions outside 0 to 1 fail with `AE_INVALID_OPTION`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsSetRelKeyPos(index: AdsHandle, position: f64) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetRelKeyPos");
    let backend = api(index);
    if let Err(error) = backend.index_info(index) {
        return error_code(error);
    }
    if !(0.0..=1.0).contains(&position) {
        return fail(AE_INVALID_OPTION);
    }
    code(backend.set_relative_position(index, position))
}

/// `error`'s code, with the scope that is not set named as the original
/// engine names it (`Top`, `Bottom`).
fn no_scope_code(error: tflip_engine::Error, bottom: bool) -> Unsigned32 {
    match error {
        tflip_engine::Error::NoScope => {
            crate::last_error::fail_detail(AE_NO_SCOPE, if bottom { "Bottom" } else { "Top" })
        }
        error => error_code(error),
    }
}

/// Whether `option` names the scope's bottom (`ADS_BOTTOM`) or its top
/// (`ADS_TOP`); `AE_INVALID_OPTION` for anything else.
fn scope_bound(option: Unsigned16) -> Result<bool, Unsigned32> {
    match option {
        ADS_TOP => Ok(false),
        ADS_BOTTOM => Ok(true),
        _ => Err(AE_INVALID_OPTION),
    }
}

/// `AdsSetScope`: limits the keys an index order shows, for moves through
/// it, seeks and key counts: from the first key not before an `ADS_TOP`
/// scope to the last not after an `ADS_BOTTOM` scope (the largest keys are
/// at the top of a descending order). The key is given as for `AdsSeek` and
/// compared over its length, so a bottom of `a` takes in `abc`. The table
/// does not move; moving in record order ignores scopes.
///
/// As in the original engine: a handle that is not an index order fails
/// with `AE_INVALID_INDEX_HANDLE`, an option or data type it does not know
/// with `AE_INVALID_OPTION`, and a value that is not one of the field's type
/// as `AdsSeek` fails; a null key of length 0 changes nothing.
///
/// # Safety
///
/// `key` must be null or hold `key_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSetScope(
    index: AdsHandle,
    option: Unsigned16,
    key: *mut Unsigned8,
    key_len: Unsigned16,
    data_type: Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetScope");
    // SAFETY: `key` is null or holds `key_len` bytes, as # Safety says.
    let key = unsafe { slice(key, usize::from(key_len)) };
    let backend = api(index);
    if let Err(error) = backend.index_info(index) {
        return error_code(error);
    }
    let bottom = match scope_bound(option) {
        Ok(bottom) => bottom,
        Err(code) => return fail_detail(code, "Parameter 2"),
    };
    let client_key = match read_key(index, key, key_len, data_type) {
        Ok(client_key) => client_key,
        Err(code) => return crate::keys::key_error(code, 3),
    };
    if key.is_none() {
        return AE_SUCCESS;
    }
    code(backend.set_scope(index, bottom, client_key))
}

/// `AdsGetScope`: the key of an index order's `ADS_TOP` or `ADS_BOTTOM`
/// scope, as `AdsSetScope` converted it, in `scope` of `*len` bytes
/// (NUL-terminated); `*len` is set to its length.
///
/// As in the original engine: an order without that scope fails with
/// `AE_NO_SCOPE` and `*len` set to 0; a buffer that is too small fails with
/// `AE_INSUFFICIENT_BUFFER` and `*len` set to the length needed including the
/// NUL, after the bytes that fit are copied; a handle that is not an index
/// order fails with `AE_INVALID_INDEX_HANDLE`, an option it does not know
/// with `AE_INVALID_OPTION`.
///
/// # Safety
///
/// `scope` must be null or hold `*len` bytes; `len` must be null or valid
/// for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetScope(
    index: AdsHandle,
    option: Unsigned16,
    scope: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetScope");
    let backend = api(index);
    if let Err(error) = backend.index_info(index) {
        return error_code(error);
    }
    let bottom = match scope_bound(option) {
        Ok(bottom) => bottom,
        Err(code) => return fail_detail(code, "Parameter 2"),
    };
    if scope.is_null() || len.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    let key = match backend.scope_key(index, bottom) {
        Ok(key) => key,
        Err(error) => {
            // SAFETY: not null, so writable, as # Safety says.
            unsafe { *len = 0 };
            return no_scope_code(error, bottom);
        }
    };
    // SAFETY: `scope` and `len` are as # Safety says.
    unsafe { write_cut(&key, scope, len) }
}

/// `AdsClearScope`: removes an index order's `ADS_TOP` or `ADS_BOTTOM`
/// scope; the other stays. As in the original engine, an order without that
/// scope fails with `AE_NO_SCOPE`, a handle that is not an index order with
/// `AE_INVALID_INDEX_HANDLE`, and an option it does not know with
/// `AE_INVALID_OPTION`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsClearScope(index: AdsHandle, option: Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsClearScope");
    let backend = api(index);
    if let Err(error) = backend.index_info(index) {
        return error_code(error);
    }
    let bottom = match scope_bound(option) {
        Ok(bottom) => bottom,
        Err(code) => return fail_detail(code, "Parameter 2"),
    };
    match backend.clear_scope(index, bottom) {
        Ok(()) => AE_SUCCESS,
        Err(error) => no_scope_code(error, bottom),
    }
}

/// `AdsClearAllScopes`: removes the scopes of every index order of a table.
/// As in the original engine, a handle that is not a table (an index order
/// among them) fails with `AE_INVALID_TABLE_HANDLE`.
#[unsafe(no_mangle)]
pub extern "system" fn AdsClearAllScopes(table: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsClearAllScopes");
    code(api(table).clear_all_scopes(table))
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use super::*;
    use crate::*;

    fn order(table: AdsHandle, name: &CStr) -> AdsHandle {
        let mut order = 0;
        let name = name.as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { AdsGetIndexHandle(table, name, &mut order) },
            AE_SUCCESS
        );
        order
    }

    fn record_number(table: AdsHandle) -> u32 {
        let mut record = 0;
        unsafe { crate::AdsGetRecordNum(table, ADS_IGNOREFILTERS, &mut record) };
        record
    }

    /// A record a conditional order has no key for has no key number, as in
    /// the original engine.
    #[test]
    fn key_numbers_of_records_without_keys() {
        let (connect, table) = fixtures::open("adi_options", c"opts.adt");
        let mut cond = 0;
        let name = c"COND".as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { AdsGetIndexHandle(table, name, &mut cond) },
            AE_SUCCESS
        );
        let mut key = 0xffff_ffff;
        // COND is `id > 30`: record 10 (id 9) has no key, record 33 the first.
        assert_eq!(crate::AdsGotoRecord(table, 10), AE_SUCCESS);
        assert_eq!(
            unsafe { AdsGetKeyNum(cond, 1, &mut key) },
            crate::AE_NOT_FOUND
        );
        assert_eq!(key, 0xffff_ffff, "left alone");
        assert_eq!(crate::AdsGotoRecord(table, 33), AE_SUCCESS);
        assert_eq!(unsafe { AdsGetKeyNum(cond, 1, &mut key) }, AE_SUCCESS);
        assert_eq!(key, 1);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    /// The arguments the scope functions check, and what `AdsGetScope`
    /// returns, as in the original engine; what scopes do is tested in the
    /// engine.
    #[test]
    fn scope_arguments() {
        // Scopes parse dates in the process-wide date format.
        let _settings = fixtures::settings_lock();
        use crate::{
            ADS_BOTTOM, ADS_DOUBLEKEY, ADS_RAWKEY, ADS_RESPECTSCOPES, ADS_STRINGKEY, ADS_TOP,
            AE_INVALID_DATE, AE_INVALID_KEY_LENGTH, AE_INVALID_OPTION, AE_INVALID_TABLE_HANDLE,
            AE_NO_SCOPE, AdsGetRecordCount, AdsGotoTop, AdsIsFound,
        };
        let (connect, table) = fixtures::open("adi_keys", c"keys.adt");
        let order = |name: &CStr| {
            let mut handle = 0;
            let name = name.as_ptr().cast_mut().cast();
            assert_eq!(
                unsafe { AdsGetIndexHandle(table, name, &mut handle) },
                AE_SUCCESS
            );
            handle
        };
        let (chars, dates) = (order(c"I_CHAR"), order(c"I_DATE"));
        let set = |index, option, key: &[u8], data_type| unsafe {
            AdsSetScope(
                index,
                option,
                key.as_ptr().cast_mut(),
                key.len() as u16,
                data_type,
            )
        };
        let get = |index, option, size| {
            let mut buf = [0xEEu8; 8];
            let mut len = size;
            let rc = unsafe { AdsGetScope(index, option, buf.as_mut_ptr(), &mut len) };
            (rc, len, buf)
        };

        let (rc, len, _) = get(chars, ADS_TOP, 8);
        assert_eq!((rc, len), (AE_NO_SCOPE, 0));
        assert_eq!(set(chars, ADS_TOP, b"abc", ADS_STRINGKEY), AE_SUCCESS);
        let (rc, len, buf) = get(chars, ADS_TOP, 4);
        assert_eq!((rc, len, &buf[..5]), (AE_SUCCESS, 3, &b"abc\0\xEE"[..]));
        // Too small: what fits is copied, and the size with the NUL asked for.
        let (rc, len, buf) = get(chars, ADS_TOP, 2);
        assert_eq!(
            (rc, len, &buf[..3]),
            (AE_INSUFFICIENT_BUFFER, 4, &b"ab\xEE"[..])
        );
        assert_eq!(get(chars, ADS_BOTTOM, 8).0, AE_NO_SCOPE);

        // Counting from the top seeks it.
        assert_eq!(AdsGotoTop(chars), AE_SUCCESS);
        let mut count = 0;
        assert_eq!(
            unsafe { AdsGetRecordCount(chars, ADS_RESPECTSCOPES, &mut count) },
            AE_SUCCESS
        );
        assert_eq!(count, 2);
        let mut found = 0;
        assert_eq!(unsafe { AdsIsFound(table, &mut found) }, AE_SUCCESS);
        assert_eq!(found, ADS_TRUE);
        assert_eq!(
            unsafe { AdsGetRecordCount(table, ADS_RESPECTSCOPES, &mut count) },
            AE_SUCCESS
        );
        assert_eq!(count, 6, "the table itself has no scope");

        assert_eq!(AdsClearScope(chars, ADS_TOP), AE_SUCCESS);
        assert_eq!(AdsClearScope(chars, ADS_TOP), AE_NO_SCOPE);
        let null = std::ptr::null_mut();
        assert_eq!(
            unsafe { AdsSetScope(chars, ADS_TOP, null, 0, ADS_STRINGKEY) },
            AE_SUCCESS
        );
        assert_eq!(
            get(chars, ADS_TOP, 8).0,
            AE_NO_SCOPE,
            "a null key sets nothing"
        );
        assert_eq!(
            unsafe { AdsSetScope(chars, ADS_TOP, null, 1, ADS_STRINGKEY) },
            AE_INVALID_POINTER
        );
        assert_eq!(set(chars, 3, b"a", ADS_STRINGKEY), AE_INVALID_OPTION);
        assert_eq!(set(chars, ADS_TOP, b"a", 3), AE_INVALID_OPTION);
        assert_eq!(
            set(chars, ADS_TOP, b"a", ADS_DOUBLEKEY),
            AE_INVALID_KEY_LENGTH
        );
        assert_eq!(
            set(dates, ADS_TOP, b"13/01/2000", ADS_STRINGKEY),
            AE_INVALID_DATE
        );
        assert_eq!(
            set(table, ADS_TOP, b"a", ADS_RAWKEY),
            AE_INVALID_INDEX_HANDLE
        );
        assert_eq!(get(chars, 3, 8).0, AE_INVALID_OPTION);
        assert_eq!(get(table, ADS_TOP, 8).0, AE_INVALID_INDEX_HANDLE);
        assert_eq!(AdsClearScope(chars, 0), AE_INVALID_OPTION);
        assert_eq!(AdsClearScope(table, ADS_TOP), AE_INVALID_INDEX_HANDLE);

        assert_eq!(
            set(dates, ADS_BOTTOM, b"01/01/1900", ADS_STRINGKEY),
            AE_SUCCESS
        );
        assert_eq!(AdsClearAllScopes(chars), AE_INVALID_TABLE_HANDLE);
        assert_eq!(AdsClearAllScopes(table), AE_SUCCESS);
        assert_eq!(get(dates, ADS_BOTTOM, 8).0, AE_NO_SCOPE);
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    fn relative(order: AdsHandle) -> Result<f64, Unsigned32> {
        let mut position = -1.0;
        match unsafe { AdsGetRelKeyPos(order, &mut position) } {
            AE_SUCCESS => Ok(position),
            code => Err(code),
        }
    }

    /// Relative positions, as the original engine computes them: the middle
    /// of a key's share, and floor(p × n) from the top (from the bottom in a
    /// descending order).
    #[test]
    fn relative_key_positions() {
        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        let by_name = order(table, c"BY_NAME");
        let desc = order(table, c"BY_AMOUNT_DESC");
        crate::AdsGotoTop(by_name);
        let mut seen = Vec::new();
        for _ in 0..5 {
            seen.push(relative(by_name));
            crate::AdsSkip(by_name, 1);
        }
        assert_eq!(
            seen,
            [Ok(0.125), Ok(0.375), Ok(0.625), Ok(0.875), Err(5068)]
        );
        for (order, position, want) in [
            (by_name, 0.25, 0.375),
            (by_name, 0.74, 0.625),
            (by_name, 1.0, 0.875),
            (desc, 0.25, 0.125),
            (desc, 0.3, 0.375),
            (desc, 0.75, 0.625),
            (desc, 0.76, 0.875),
        ] {
            assert_eq!(AdsSetRelKeyPos(order, position), AE_SUCCESS);
            assert_eq!(relative(order), Ok(want), "{position}");
        }
        assert_eq!(AdsSetRelKeyPos(by_name, 2.0), AE_INVALID_OPTION);
        assert_eq!(AdsSetRelKeyPos(table, 0.5), AE_INVALID_INDEX_HANDLE);
        assert_eq!(
            unsafe { AdsGetRelKeyPos(by_name, std::ptr::null_mut()) },
            AE_INVALID_POINTER
        );
        crate::AdsDisconnect(connect);
    }

    /// `AdsSkipUnique`, `AdsSeekLast` and `AdsSetIndexDirection` on an order
    /// with duplicate keys (`adi_keys`' Logical order: NULL, F, F, T, T, T).
    #[test]
    fn unique_skips_last_seeks_and_directions() {
        let (connect, table) = fixtures::open("adi_keys", c"keys.adt");
        let logical = order(table, c"I_LOGICAL");
        crate::AdsGotoTop(logical);
        let mut seen = vec![record_number(table)];
        for _ in 0..3 {
            assert_eq!(AdsSkipUnique(logical, 1), AE_SUCCESS);
            seen.push(record_number(table));
        }
        assert_eq!(seen, [6, 2, 1, 7]);
        let mut back = Vec::new();
        for _ in 0..2 {
            assert_eq!(AdsSkipUnique(logical, -1), AE_SUCCESS);
            back.push(record_number(table));
        }
        assert_eq!(back, [5, 3], "the last record of each key");
        assert_eq!(AdsSkipUnique(table, 1), AE_INVALID_INDEX_HANDLE);

        let seek_last = |key: &CStr| {
            let mut found = 0x77;
            let rc =
                unsafe { AdsSeekLast(logical, key.as_ptr().cast_mut().cast(), 1, 1, &mut found) };
            assert_eq!(rc, AE_SUCCESS);
            (found, record_number(table))
        };
        assert_eq!(seek_last(c"T"), (ADS_TRUE, 5));
        assert_eq!(seek_last(c"F"), (ADS_TRUE, 3));

        assert_eq!(AdsSetIndexDirection(logical, 1), AE_SUCCESS);
        crate::AdsGotoTop(logical);
        assert_eq!(
            (record_number(table), relative(logical)),
            (5, Ok(1.0 / 12.0))
        );
        crate::AdsGotoBottom(logical);
        assert_eq!(record_number(table), 6);
        assert_eq!(AdsSetIndexDirection(logical, 2), AE_SUCCESS);
        crate::AdsGotoTop(logical);
        assert_eq!(record_number(table), 6);
        crate::AdsDisconnect(connect);

        let (connect, table) = fixtures::open("indexes", c"indexed.adt");
        let by_name = order(table, c"BY_NAME");
        let seek_last = |key: &CStr| {
            let (mut found, mut bof, mut eof) = (0x77, 0, 0);
            let length = key.to_bytes().len() as Unsigned16;
            let key = key.as_ptr().cast_mut().cast();
            assert_eq!(
                unsafe { AdsSeekLast(by_name, key, length, 1, &mut found) },
                AE_SUCCESS
            );
            unsafe { crate::AdsAtBOF(table, &mut bof) };
            unsafe { crate::AdsAtEOF(table, &mut eof) };
            (found, bof, eof)
        };
        assert_eq!(seek_last(c"B"), (ADS_TRUE, 0, 0));
        assert_eq!(
            seek_last(c"Zulu"),
            (ADS_FALSE, 0, 1),
            "past the last key: EOF"
        );
        assert_eq!(
            seek_last(c"0"),
            (ADS_FALSE, 1, 1),
            "before the first: both ends"
        );
        crate::AdsDisconnect(connect);
    }

    /// As in the original engine: key counts, record counts, key numbers,
    /// seeks and seek-lasts in an order with deleted records shown and hidden
    /// (`fixtures/seek/deleted.json`, from `tools/ace/seek/deleted.c`, whose
    /// steps this repeats).
    #[test]
    fn deleted_records_in_orders() {
        use serde_json::{Value as Json, json};
        let _settings = crate::fixtures::settings_lock();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/seek/deleted.json");
        let want: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, _dir) =
            crate::fixtures::scratch_from("deleted_in_orders", &crate::fixtures::empty_dir());
        let connect = crate::fixtures::connect_to(&source);
        let mut got: Vec<Json> = Vec::new();
        let text = |s: &str| std::ffi::CString::new(s).unwrap();
        let at = |table: AdsHandle| -> Json {
            let (mut bof, mut eof) = (0, 0);
            unsafe { AdsAtBOF(table, &mut bof) };
            unsafe { AdsAtEOF(table, &mut eof) };
            match (bof, eof) {
                (_, 1) => json!("E"),
                (1, _) => json!("B"),
                _ => json!(record_number(table)),
            }
        };
        type CountFn =
            unsafe extern "system" fn(AdsHandle, Unsigned16, *mut Unsigned32) -> Unsigned32;
        let types = [
            ("ADT", "ta.adt", "ta.adi", ADS_ADT),
            ("NTX", "tn.dbf", "tn_k.ntx", ADS_NTX),
            ("CDX", "tc.dbf", "tc.cdx", ADS_CDX),
            ("VFP", "tv.dbf", "tv.cdx", ADS_VFP),
        ];
        for (type_name, table_name, bag, table_type) in types {
            let (mut table, mut order) = (0, 0);
            AdsShowDeleted(1);
            let (name, fields) = (text(table_name), text("K,C,8,0"));
            let null = std::ptr::null_mut();
            let rc = unsafe {
                crate::AdsCreateTable(
                    connect,
                    name.as_ptr().cast_mut().cast(),
                    null,
                    table_type,
                    1,
                    1,
                    1,
                    0,
                    fields.as_ptr().cast_mut().cast(),
                    &mut table,
                )
            };
            assert_eq!(rc, AE_SUCCESS);
            let field = text("K");
            for value in ["A1", "A2", "A3", "B1", "B2", "B3", "C1", "C2"] {
                let value = text(value);
                crate::AdsAppendRecord(table);
                unsafe {
                    crate::AdsSetString(
                        table,
                        field.as_ptr().cast_mut().cast(),
                        value.as_ptr().cast_mut().cast(),
                        2,
                    )
                };
                crate::AdsWriteRecord(table);
            }
            let (bag, empty) = (text(bag), text(""));
            let rc = unsafe {
                crate::AdsCreateIndex61(
                    table,
                    bag.as_ptr().cast_mut().cast(),
                    field.as_ptr().cast_mut().cast(),
                    field.as_ptr().cast_mut().cast(),
                    empty.as_ptr().cast_mut().cast(),
                    empty.as_ptr().cast_mut().cast(),
                    0,
                    512,
                    &mut order,
                )
            };
            assert_eq!(rc, AE_SUCCESS);
            for gone in [1, 6, 7, 8] {
                crate::AdsGotoRecord(table, gone);
                crate::AdsDeleteRecord(table);
                crate::AdsWriteRecord(table);
            }
            let count = |show: i32, op: &str, function: CountFn, handle, option: u16| {
                let mut n = 777;
                let rc = unsafe { function(handle, option, &mut n) };
                json!({"type": type_name, "show": show, "op": op, "option": option,
                       "rc": rc, "n": n})
            };
            for show in [1, 0] {
                AdsShowDeleted(show as Unsigned16);
                for option in 1..=3 {
                    got.push(count(show, "keycount", AdsGetKeyCount, order, option));
                }
                for option in 1..=3 {
                    got.push(count(
                        show,
                        "recordcount",
                        crate::AdsGetRecordCount,
                        table,
                        option,
                    ));
                }
                crate::AdsGotoTop(order);
                for option in 1..=3 {
                    got.push(count(show, "keynum", AdsGetKeyNum, order, option));
                }
                let b = text("B");
                unsafe { AdsSetScope(order, 1, b.as_ptr().cast_mut().cast(), 1, 2) };
                unsafe { AdsSetScope(order, 2, b.as_ptr().cast_mut().cast(), 1, 2) };
                for option in 1..=3 {
                    got.push(count(
                        show,
                        "scoped keycount",
                        AdsGetKeyCount,
                        order,
                        option,
                    ));
                }
                AdsClearScope(order, 1);
                AdsClearScope(order, 2);
                let seeks = |prefix: &str| -> Vec<Json> {
                    let mut rows = Vec::new();
                    for key in ["A", "B", "C", "D", "0", "A1", "A2", "B3", "C1", "B2"] {
                        let k = text(key);
                        let k = k.as_ptr().cast_mut().cast();
                        let len = key.len() as Unsigned16;
                        for seek_type in [1u16, 2, 4] {
                            let mut found = 7;
                            crate::AdsGotoRecord(table, 2);
                            let rc = unsafe { AdsSeek(order, k, len, 2, seek_type, &mut found) };
                            rows.push(json!({"type": type_name, "show": show,
                                             "op": format!("{prefix}seek"), "key": key,
                                             "seek_type": seek_type, "rc": rc,
                                             "found": found, "at": at(table)}));
                        }
                        let mut found = 7;
                        crate::AdsGotoRecord(table, 2);
                        let rc = unsafe { AdsSeekLast(order, k, len, 2, &mut found) };
                        rows.push(json!({"type": type_name, "show": show,
                                         "op": format!("{prefix}seeklast"), "key": key,
                                         "rc": rc, "found": found, "at": at(table)}));
                    }
                    rows
                };
                got.extend(seeks(""));
                let aof = text("K <> 'A2'");
                unsafe { crate::AdsSetAOF(table, aof.as_ptr().cast_mut().cast(), 2) };
                for option in 1..=3 {
                    got.push(count(show, "aof keycount", AdsGetKeyCount, order, option));
                }
                crate::AdsGotoTop(order);
                for option in 1..=3 {
                    got.push(count(show, "aof keynum", AdsGetKeyNum, order, option));
                }
                unsafe { AdsSetScope(order, 1, b.as_ptr().cast_mut().cast(), 1, 2) };
                unsafe { AdsSetScope(order, 2, b.as_ptr().cast_mut().cast(), 1, 2) };
                for option in 1..=3 {
                    got.push(count(
                        show,
                        "aof scoped keycount",
                        AdsGetKeyCount,
                        order,
                        option,
                    ));
                }
                AdsClearScope(order, 1);
                AdsClearScope(order, 2);
                crate::AdsClearAOF(table);
                let hide = text("K <> 'A1' .AND. K <> 'B3'");
                unsafe { crate::AdsSetAOF(table, hide.as_ptr().cast_mut().cast(), 2) };
                got.extend(seeks("aof "));
                crate::AdsClearAOF(table);
                unsafe { crate::AdsSetFilter(table, hide.as_ptr().cast_mut().cast()) };
                got.extend(seeks("filter "));
                crate::AdsClearFilter(table);
                let mut rows = Vec::new();
                crate::AdsGotoTop(order);
                for _ in 0..20 {
                    let mut eof = 0;
                    unsafe { AdsAtEOF(table, &mut eof) };
                    if eof != 0 {
                        break;
                    }
                    rows.push(record_number(table));
                    crate::AdsSkip(order, 1);
                }
                got.push(json!({"type": type_name, "show": show, "op": "walk", "rows": rows}));
            }
            crate::AdsGotoRecord(table, 2);
            crate::AdsDeleteRecord(table);
            got.push(count(0, "keycount after delete", AdsGetKeyCount, order, 1));
            crate::AdsCloseTable(table);
        }
        AdsShowDeleted(1);
        crate::AdsDisconnect(connect);
        let problems: Vec<String> = want
            .iter()
            .zip(&got)
            .filter(|(want, got)| want != got)
            .map(|(want, got)| format!("want {want}\n got {got}"))
            .collect();
        assert_eq!(want.len(), got.len());
        assert!(
            problems.is_empty(),
            "{} of {} differ:\n{}",
            problems.len(),
            want.len(),
            problems.join("\n")
        );
    }

    /// As in the original engine: `AdsSetIndexDirection` with any value but
    /// 0 turns an ADT, CDX or VFP order round from how it is walked now, and
    /// refuses an NTX order with 5017 (`fixtures/seek/direction.json`, from
    /// `tools/ace/seek/direction.c`, whose steps this repeats).
    #[test]
    fn index_directions() {
        use serde_json::{Value as Json, json};
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/seek/direction.json");
        let want: Vec<Json> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (source, _dir) = fixtures::scratch_from("index_directions", &fixtures::empty_dir());
        let connect = fixtures::connect_to(&source);
        let types = [
            ("ADT", "adi", crate::ADS_ADT),
            ("NTX", "ntx", crate::ADS_NTX),
            ("CDX", "cdx", crate::ADS_CDX),
            ("VFP", "cdx", crate::ADS_VFP),
        ];
        let kinds = [
            ("ascending", 0, false),
            ("descending", 8, false),
            ("compound ascending", 2, true),
            ("compound descending", 10, true),
        ];
        let walk = |table: AdsHandle, mover: AdsHandle| {
            let mut records = Vec::new();
            crate::AdsGotoTop(mover);
            for _ in 0..5 {
                let mut eof = 0;
                unsafe { crate::AdsAtEOF(table, &mut eof) };
                if eof != 0 {
                    break;
                }
                records.push(record_number(table));
                crate::AdsSkip(mover, 1);
            }
            records
        };
        let mut got = Vec::new();
        for (type_name, ext, table_type) in types {
            let name = std::ffi::CString::new(format!("d{table_type}")).unwrap();
            let mut table = 0;
            let rc = unsafe {
                crate::AdsCreateTable(
                    connect,
                    name.as_ptr().cast_mut().cast(),
                    std::ptr::null_mut(),
                    table_type,
                    1,
                    0,
                    1,
                    0,
                    c"C,C,4,0".as_ptr().cast_mut().cast(),
                    &mut table,
                )
            };
            assert_eq!(rc, AE_SUCCESS);
            for value in [c"BBBB", c"AAAA", c"CCCC"] {
                crate::AdsAppendRecord(table);
                unsafe {
                    crate::AdsSetString(
                        table,
                        c"C".as_ptr().cast_mut().cast(),
                        value.as_ptr().cast_mut().cast(),
                        4,
                    )
                };
                crate::AdsWriteRecord(table);
            }
            for (k, (kind, options, compound)) in kinds.into_iter().enumerate() {
                if (table_type == crate::ADS_ADT && !compound)
                    || (table_type == crate::ADS_NTX && compound)
                {
                    continue;
                }
                let bag = std::ffi::CString::new(format!("o{table_type}_{k}.{ext}")).unwrap();
                let mut order = 0;
                let rc = unsafe {
                    crate::AdsCreateIndex61(
                        table,
                        bag.as_ptr().cast_mut().cast(),
                        c"K".as_ptr().cast_mut().cast(),
                        c"C".as_ptr().cast_mut().cast(),
                        c"".as_ptr().cast_mut().cast(),
                        c"".as_ptr().cast_mut().cast(),
                        options,
                        512,
                        &mut order,
                    )
                };
                assert_eq!(rc, AE_SUCCESS);
                for value in [1u16, 1, 0, 2, 1, 0] {
                    let rc = AdsSetIndexDirection(order, value);
                    let (mut error, mut text, mut len) = (0, [0u8; 600], 600u16);
                    unsafe { crate::AdsGetLastError(&mut error, text.as_mut_ptr(), &mut len) };
                    let mut descending = 7;
                    unsafe { crate::AdsIsIndexDescending(order, &mut descending) };
                    let order_walk = walk(table, order);
                    let table_walk = walk(table, table);
                    let mut key = 0;
                    crate::AdsGotoTop(order);
                    unsafe { crate::AdsGetKeyNum(order, 2, &mut key) };
                    got.push(json!({"type": type_name, "kind": kind, "value": value,
                                    "rc": rc, "error": error, "descending": descending,
                                    "walk": order_walk, "table_walk": table_walk,
                                    "top_key": key}));
                }
                crate::AdsCloseIndex(order);
            }
            crate::AdsCloseTable(table);
        }
        crate::AdsDisconnect(connect);
        let problems: Vec<String> = want
            .iter()
            .zip(&got)
            .filter(|(want, got)| want != got)
            .map(|(want, got)| format!("want {want}\n got {got}"))
            .collect();
        assert_eq!(want.len(), got.len());
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    crate::fixtures::remote_twins!(
        key_numbers_of_records_without_keys,
        index_directions,
        deleted_records_in_orders,
        scope_arguments,
        relative_key_positions,
        unique_skips_last_seeks_and_directions,
    );
}
