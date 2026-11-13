//! Positions in index orders: scopes, key numbers and counts, relative key
//! positions, skipping duplicate keys, and the direction of an order.

use crate::backend::api;
use crate::ffi::slice;
use crate::field::write_at;
use crate::keys::read_key;
use crate::last_error::{fail, fail_detail};
use crate::{
    ADS_BOTTOM, ADS_IGNOREFILTERS, ADS_TOP, ADS_TRUE, AE_INVALID_FILTER_OPTION, AE_INVALID_OPTION,
    AE_INVALID_POINTER, AE_NO_SCOPE, AE_SUCCESS, AdsHandle, Signed32, Unsigned8, Unsigned16,
    Unsigned32, code, error_code, write_cut,
};

/// `AdsGetKeyNum`: the current record's place in an index order, 1-based:
/// 0 at BOF and one past the last key at EOF, as in the original engine. On a
/// record the order has no key for it fails with `AE_NOT_FOUND`, leaving
/// `*key` alone. Unless `filter_option` is `ADS_IGNOREFILTERS`, the place
/// counts from the order's scope (see
/// [`tflip_engine::table::Table::key_number_in`]); there are no filters
/// yet.
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
    let scoped = filter_option != ADS_IGNOREFILTERS;
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
/// Unless `filter_option` is `ADS_IGNOREFILTERS`, only the keys in the
/// order's scope count; there are no filters yet.
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
    let scoped = filter_option != ADS_IGNOREFILTERS;
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

/// `AdsSetIndexDirection`: walks an index order from its last key to its
/// first (`reverse` 1) or as it is (anything else, as the original engine
/// takes 2); key numbers and relative positions follow.
#[unsafe(no_mangle)]
pub extern "system" fn AdsSetIndexDirection(index: AdsHandle, reverse: Unsigned16) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSetIndexDirection");
    code(api(index).set_index_direction(index, reverse == ADS_TRUE))
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

    crate::fixtures::remote_twins!(
        key_numbers_of_records_without_keys,
        scope_arguments,
        relative_key_positions,
        unique_skips_last_seeks_and_directions,
    );
}
