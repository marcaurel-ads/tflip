//! Keys of index orders: seeking them (`AdsSeek`, `AdsSeekLast`), the
//! current record's key (`AdsExtractKey`), building keys from values or
//! records and raw keys from parts (`AdsBuildRawKey`), looking keys up,
//! stepping through them, and adding and deleting keys of custom orders.

use tflip_engine::getters::Charset;
use tflip_engine::index::IndexInfo;
use tflip_engine::seek::SeekType;
use tflip_protocol::Key;

use crate::backend::api;
use crate::ffi::{slice, text, write_string};
use crate::last_error::fail;
use crate::{
    ADS_DOUBLEKEY, ADS_FALSE, ADS_HARDSEEK, ADS_RAWKEY, ADS_SEEKGT, ADS_SOFTSEEK, ADS_STRINGKEY,
    ADS_TRUE, ADS_WSTRINGKEY, AE_INVALID_KEY_LENGTH, AE_INVALID_OPTION, AE_INVALID_POINTER,
    AE_SUCCESS, AdsHandle, Unsigned8, Unsigned16, Unsigned32, ansi, code, error_code,
};

/// Reads a key as a client gives it to `AdsSeek` or `AdsSetScope` for the
/// index order `index`: the `len` bytes at `key`, of the `ADS_*KEY` type
/// `data_type`. Text is taken as the order's keys hold it: sought in an OEM
/// table (a Clipper or FoxPro table opened with `ADS_OEM`), converted to the
/// client's OEM code page, as the original engine converts it (`Ä` finds the
/// `8E` the table holds); in an order on an NChar or NVarChar field, as
/// Unicode (ANSI text read in the client's ANSI code page). Fails with the
/// ACE code: `AE_INVALID_OPTION` for a type it does not know,
/// `AE_INVALID_POINTER` for a null key that is not empty, and
/// `AE_INVALID_KEY_LENGTH` for a double that is not 8 bytes. `key` is
/// `None` for a null pointer, else its `len` bytes.
pub(crate) fn read_key(
    index: AdsHandle,
    key: Option<&[u8]>,
    len: Unsigned16,
    data_type: Unsigned16,
) -> Result<Key, Unsigned32> {
    if ![ADS_RAWKEY, ADS_STRINGKEY, ADS_DOUBLEKEY, ADS_WSTRINGKEY].contains(&data_type) {
        return Err(AE_INVALID_OPTION);
    }
    let bytes: &[u8] = match (key, len) {
        (None, 0) => &[],
        (None, _) => return Err(AE_INVALID_POINTER),
        (Some(bytes), _) => bytes,
    };
    let charset = || order_charset(index);
    Ok(match data_type {
        ADS_RAWKEY => Key::Raw(bytes.to_vec()),
        ADS_STRINGKEY => Key::Text(match charset() {
            Charset::Oem => ansi::ansi_to_oem(&ansi::data_in(bytes)),
            Charset::Unicode => ansi::to_string(bytes),
            Charset::Ansi => ansi::data_in(bytes),
        }),
        ADS_WSTRINGKEY => {
            let units: Vec<u16> = bytes
                .as_chunks()
                .0
                .iter()
                .map(|&unit| u16::from_le_bytes(unit))
                .collect();
            let text = String::from_utf16_lossy(&units);
            Key::Text(match charset() {
                Charset::Oem => ansi::unicode_to_oem(&text)
                    .unwrap_or_else(|| ansi::ansi_to_oem(&ansi::data_from_unicode(&text))),
                Charset::Unicode => text,
                Charset::Ansi => ansi::data_from_unicode(&text),
            })
        }
        _ => match <[u8; 8]>::try_from(bytes) {
            Ok(double) => Key::Double(f64::from_le_bytes(double)),
            Err(_) => return Err(AE_INVALID_KEY_LENGTH),
        },
    })
}

/// Fails with `code` of [`read_key`] as the original engine words it:
/// `Parameter N` for a null key, the data type's name for a double that is not
/// 8 bytes (`fixtures/errors/param_n`). `position` is the number of the
/// argument the key is.
pub(crate) fn key_error(code: Unsigned32, position: u32) -> Unsigned32 {
    match code {
        AE_INVALID_POINTER => crate::last_error::null_param(position),
        AE_INVALID_KEY_LENGTH => {
            crate::last_error::fail_detail(code, "Invalid seek length for ADS_DOUBLEKEY.")
        }
        _ => crate::last_error::fail(code),
    }
}

/// How the index order `index` holds text: as Unicode for an order on an
/// NChar or NVarChar field, in the client's OEM code page for an OEM table
/// (see [`tflip_engine::getters::oem_table`]), else in its ANSI code page.
fn order_charset(index: AdsHandle) -> Charset {
    let backend = api(index);
    let Ok(info) = backend.index_info(index) else {
        return Charset::Ansi;
    };
    let Ok(table) = backend.table_info(info.table) else {
        return Charset::Ansi;
    };
    let structure = &table.structure;
    if let [number] = &info.segments[..]
        && let Some(field) = structure.fields.get(number.wrapping_sub(1))
        && tflip_engine::getters::charset_in(&table.settings, structure, field) == Charset::Unicode
    {
        return Charset::Unicode;
    }
    match tflip_engine::getters::oem_table(&table.settings, structure) {
        true => Charset::Oem,
        false => Charset::Ansi,
    }
}

/// `AdsSeek`: moves the table of an index order to the first record whose
/// key starts with `key` (`key_len` bytes), and sets `*found`. `data_type`
/// says what `key` holds: the key's bytes (`ADS_RAWKEY`), ANSI or UTF-16 text
/// (`ADS_STRINGKEY`, `ADS_WSTRINGKEY`, converted as the order's field is
/// typed; dates follow `AdsSetDateFormat`), or a double (`ADS_DOUBLEKEY`, 8
/// bytes). `seek_type` says what happens to a key the order does not have:
/// the table lands on the next key (`ADS_SOFTSEEK`) or at EOF
/// (`ADS_HARDSEEK`); `ADS_SEEKGT` lands after every key that starts with
/// `key`. See [`tflip_engine::table::Table::seek_in`] and
/// [`tflip_engine::seek`].
///
/// As in the original engine: a handle that is not an index order fails
/// with `AE_INVALID_INDEX_HANDLE`, a null `found` with `AE_INVALID_POINTER`,
/// leaving `*found` alone; otherwise `*found` is `ADS_FALSE` on failure. A
/// data or seek type it does not know fails with `AE_INVALID_OPTION`, a
/// double that is not 8 bytes with `AE_INVALID_KEY_LENGTH`, and a value that
/// is not one of the field's type with `AE_INVALID_DATE`,
/// `AE_INVALID_DATA_TYPE`, `AE_VALUE_OVERFLOW` and the like. Text sought in
/// an NChar or NVarChar order is its ICU sort key, whole (see
/// [`tflip_storage::icu`]); text with a character whose weights tflip does
/// not know fails with `AE_NOT_IMPLEMENTED`.
///
/// # Safety
///
/// `key` must be null or hold `key_len` bytes; `found` must be null or valid
/// for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSeek(
    index: AdsHandle,
    key: *mut Unsigned8,
    key_len: Unsigned16,
    data_type: Unsigned16,
    seek_type: Unsigned16,
    found: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSeek");
    // SAFETY: `key` is null or holds `key_len` bytes, `found` is null or
    // writable, as # Safety says.
    let (key, found) = unsafe { (slice(key, usize::from(key_len)), found.as_mut()) };
    let backend = api(index);
    if let Err(error) = backend.index_info(index) {
        return error_code(error);
    }
    let Some(found) = found else {
        return crate::last_error::null_param(6);
    };
    *found = ADS_FALSE;
    let how = match seek_type {
        ADS_SOFTSEEK => SeekType::Soft,
        ADS_HARDSEEK => SeekType::Hard,
        ADS_SEEKGT => SeekType::After,
        _ => return fail(AE_INVALID_OPTION),
    };
    let key = match read_key(index, key, key_len, data_type) {
        Ok(key) => key,
        Err(code) => return key_error(code, 2),
    };
    match backend.seek(index, key, how) {
        Ok(true) => {
            *found = ADS_TRUE;
            AE_SUCCESS
        }
        Ok(false) => AE_SUCCESS,
        Err(error) => error_code(error),
    }
}

/// `AdsSeekLast`: as `AdsSeek` with `ADS_HARDSEEK`, but to the last record
/// whose key starts with `key`; not found, the table is at EOF.
///
/// # Safety
///
/// `key` must be null or point to a key as `AdsSeek` takes it; `found` must
/// be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsSeekLast(
    index: AdsHandle,
    key: *mut Unsigned8,
    key_len: Unsigned16,
    data_type: Unsigned16,
    found: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsSeekLast");
    // SAFETY: `key` is null or holds `key_len` bytes, `found` is null or
    // writable, as # Safety says.
    let (key, found) = unsafe { (slice(key, usize::from(key_len)), found.as_mut()) };
    let backend = api(index);
    if let Err(error) = backend.index_info(index) {
        return error_code(error);
    }
    let Some(found) = found else {
        return crate::last_error::null_param(6);
    };
    *found = ADS_FALSE;
    let key = match read_key(index, key, key_len, data_type) {
        Ok(key) => key,
        Err(code) => return fail(code),
    };
    match backend.seek_last(index, key) {
        Ok(true) => {
            *found = ADS_TRUE;
            AE_SUCCESS
        }
        Ok(false) => AE_SUCCESS,
        Err(error) => error_code(error),
    }
}

/// `AdsExtractKey`: the key of the current record in an index order, in
/// `key` of `*len` bytes (no NUL); `*len` is set to its length. As in the
/// original engine, the order's condition does not matter, EOF gives an
/// empty record's key, and a buffer that is too small gets what fits and
/// `AE_INSUFFICIENT_BUFFER`, with `*len` set to the length needed. (At BOF
/// the original engine gives the key of the record it was on before; tflip
/// gives an empty record's.) Orders whose keys are collation weights fail
/// with `AE_NOT_IMPLEMENTED`.
///
/// # Safety
///
/// `key` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsExtractKey(
    index: AdsHandle,
    key: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsExtractKey");
    let backend = api(index);
    if let Err(error) = backend.index_info(index) {
        return error_code(error);
    }
    if let Some(code) = crate::ffi::null_buffer(key, len, 2) {
        return code;
    }
    let bytes = match backend.extract_key(index) {
        Ok(bytes) => bytes,
        Err(error) => return error_code(error),
    };
    // SAFETY: neither is null, so `key` holds `*len` bytes and `len` is
    // readable and writable, as # Safety says.
    unsafe { copy_key(&bytes, key, len) }
}

/// Copies `bytes`, a key, to `key` of `*len` bytes as `AdsExtractKey` does:
/// what fits, with `*len` set to the key's length, and
/// `AE_INSUFFICIENT_BUFFER` if it did not all fit.
///
/// # Safety
///
/// `key` must hold `*len` bytes; `len` must be valid for reads and writes.
unsafe fn copy_key(bytes: &[u8], key: *mut Unsigned8, len: *mut Unsigned16) -> Unsigned32 {
    // SAFETY: as the caller promises; the copy is at most `*len` bytes.
    unsafe {
        let fits = bytes.len().min(usize::from(*len));
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), key, fits);
        let needed = bytes.len() as Unsigned16;
        let short = needed > *len;
        *len = needed;
        if short {
            return fail(crate::AE_INSUFFICIENT_BUFFER);
        }
    }
    AE_SUCCESS
}

/// `AdsLookupKey`: whether an index order has the key `key` (`key_len`
/// bytes of the `ADS_*KEY` type `data_type`, read as `AdsSeek` reads it),
/// the whole key, whatever the order's scope and the table's filters; the
/// table does not move.
///
/// # Safety
///
/// `key` must be null or hold `key_len` bytes; `found` must be null or valid
/// for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsLookupKey(
    index: AdsHandle,
    key: *mut Unsigned8,
    key_len: Unsigned16,
    data_type: Unsigned16,
    found: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsLookupKey");
    // SAFETY: `key` is null or holds `key_len` bytes, `found` is null or
    // writable, as # Safety says.
    let (key, found) = unsafe { (slice(key, usize::from(key_len)), found.as_mut()) };
    let backend = api(index);
    if let Err(error) = backend.index_info(index) {
        return error_code(error);
    }
    let Some(found) = found else {
        return fail(AE_INVALID_POINTER);
    };
    let key = match read_key(index, key, key_len, data_type) {
        Ok(key) => key,
        Err(code) => return fail(code),
    };
    match backend.lookup_key(index, key) {
        Ok(has) => {
            *found = if has { ADS_TRUE } else { ADS_FALSE };
            AE_SUCCESS
        }
        Err(error) => error_code(error),
    }
}

/// `AdsBuildKeyFromRecord` (unpublished): the key the record at `record`
/// (`record_len` bytes, the table's record length or
/// `AE_INVALID_RECORD_LENGTH`) has in an index order, whatever its
/// condition, in `key` of `*len` bytes (as `AdsExtractKey` returns a key).
///
/// # Safety
///
/// `record` must be null or hold `record_len` bytes; `key` must be null or
/// hold `*len` bytes; `len` must be null or valid for reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsBuildKeyFromRecord(
    index: AdsHandle,
    record: *mut Unsigned8,
    record_len: Unsigned32,
    key: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsBuildKeyFromRecord");
    // SAFETY: `record` is null or holds `record_len` bytes, as # Safety says.
    let record = unsafe { slice(record, record_len as usize) };
    let backend = api(index);
    if let Err(error) = backend.index_info(index) {
        return error_code(error);
    }
    let Some(record) = record else {
        return fail(AE_INVALID_POINTER);
    };
    if key.is_null() || len.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    match backend.key_from_record(index, record.to_vec()) {
        // SAFETY: neither is null, so `key` holds `*len` bytes and `len` is
        // readable and writable, as # Safety says.
        Ok(bytes) => unsafe { copy_key(&bytes, key, len) },
        Err(tflip_engine::Error::InvalidRecordLength) => {
            crate::last_error::fail_detail(crate::AE_INVALID_RECORD_LENGTH, "Building key.")
        }
        Err(error) => error_code(error),
    }
}

/// `AdsInitRawKey`: starts a raw key on an index order, from a record of
/// NULL fields; `AdsSet*` calls on the order's handle then set its fields,
/// and `AdsBuildRawKey` builds the key.
#[unsafe(no_mangle)]
pub extern "system" fn AdsInitRawKey(index: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsInitRawKey");
    let backend = api(index);
    if let Err(error) = backend.index_info(index) {
        return error_code(error);
    }
    code(backend.init_raw_key(index))
}

/// `AdsBuildRawKey`: [`AdsBuildRawKey100`] with
/// `ADS_GET_DEFAULT_KEY_LENGTH`.
///
/// # Safety
///
/// As for [`AdsBuildRawKey100`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsBuildRawKey(
    index: AdsHandle,
    key: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsBuildRawKey");
    // SAFETY: the same contract.
    unsafe { AdsBuildRawKey100(index, key, len, 0) }
}

/// `AdsBuildRawKey100`: the raw key built from the fields set on an index
/// order since `AdsInitRawKey`, in `key` of `*len` bytes. As in the original
/// engine, fields not set are NULL, and the key ends after the last segment
/// set: after the text given to it (`ADS_GET_DEFAULT_KEY_LENGTH`), after the
/// whole segment (`ADS_GET_PARTIAL_FULL_KEY_LENGTH`), or the key is whole
/// (`ADS_GET_FULL_KEY_LENGTH`); with no segment set, it is empty.
///
/// # Safety
///
/// `key` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsBuildRawKey100(
    index: AdsHandle,
    key: *mut Unsigned8,
    len: *mut Unsigned16,
    options: Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsBuildRawKey100");
    let backend = api(index);
    if let Err(error) = backend.index_info(index) {
        return error_code(error);
    }
    if key.is_null() || len.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    match backend.build_raw_key(index, options) {
        // SAFETY: neither is null, so `key` holds `*len` bytes and `len` is
        // readable and writable, as # Safety says.
        Ok(bytes) => unsafe { copy_key(&bytes, key, len) },
        Err(tflip_engine::Error::InvalidOption) => {
            crate::last_error::fail_detail(crate::AE_INVALID_OPTION, "Parameter 4")
        }
        Err(error) => error_code(error),
    }
}

/// The key after `key` (`forward`) or before it, as `AdsStepIndexKey` steps
/// it: its bytes as one big-endian number, one up or down; a key of all
/// 0xFF (or all zeros) stays.
fn step_key(key: &mut [u8], forward: bool) {
    let (edge, wrapped) = if forward { (0xFF, 0x00) } else { (0x00, 0xFF) };
    if key.iter().all(|&b| b == edge) {
        return;
    }
    for byte in key.iter_mut().rev() {
        if *byte != edge {
            *byte = if forward { *byte + 1 } else { *byte - 1 };
            return;
        }
        *byte = wrapped;
    }
}

/// `AdsStepIndexKey` (unpublished): steps the `len` bytes of the raw key at
/// `key`, in place, to the next possible key (`direction` > 0) or the one
/// before (< 0), as the original engine does (see `step_key`); a
/// `direction` of 0 fails with `AE_INVALID_OPTION`.
///
/// # Safety
///
/// `key` must be null or hold `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsStepIndexKey(
    index: AdsHandle,
    key: *mut Unsigned8,
    len: Unsigned16,
    direction: i16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsStepIndexKey");
    if let Err(error) = api(index).index_info(index) {
        return error_code(error);
    }
    if direction == 0 {
        return crate::last_error::fail_detail(AE_INVALID_OPTION, "Parameter 4");
    }
    if key.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: not null, so `key` holds `len` bytes, as # Safety says, which
    // nothing else refers to during the call.
    let key = unsafe { std::slice::from_raw_parts_mut(key, usize::from(len)) };
    step_key(key, direction > 0);
    AE_SUCCESS
}

/// `AdsGetKeyColumn`: the names of a table's (or cursor's) most unique
/// columns, each followed by `;`, in `buf` of `*len` bytes: its primary
/// key's, else those of its unique orders on one field, else its AutoInc
/// fields, else its RowVersion fields; none fails with `AE_NOT_FOUND`. A
/// buffer that is too small fails with `AE_INSUFFICIENT_BUFFER`, `*len` set
/// to the length needed with the NUL.
///
/// # Safety
///
/// `buf` must be null or hold `*len` bytes; `len` must be null or valid for
/// reads and writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetKeyColumn(
    table: AdsHandle,
    buf: *mut Unsigned8,
    len: *mut Unsigned16,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetKeyColumn");
    let backend = match crate::filter::table_of(table) {
        Ok(backend) => backend,
        Err(code) => return code,
    };
    let info = match backend.table_info(table) {
        Ok(info) => info,
        Err(error) => return error_code(error),
    };
    let fields = &info.structure.fields[..];
    let orders: Vec<IndexInfo> = backend
        .indexes(table)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|order| backend.index_info(order).ok())
        .collect();
    let single = |order: &IndexInfo| match order.segments[..] {
        [number]
            if order
                .expression
                .eq_ignore_ascii_case(&fields[number - 1].name) =>
        {
            Some(fields[number - 1].name.clone())
        }
        _ => None,
    };
    let primary: Vec<String> = orders
        .iter()
        .filter(|o| o.primary_key)
        .filter_map(single)
        .collect();
    let unique: Vec<String> = orders
        .iter()
        .filter(|o| o.unique)
        .filter_map(single)
        .collect();
    let of_type = |field_type| -> Vec<String> {
        fields
            .iter()
            .filter(|f| f.field_type == field_type)
            .map(|f| f.name.clone())
            .collect()
    };
    use tflip_storage::field::FieldType::{AutoInc, RowVersion};
    let names = [primary, unique, of_type(AutoInc), of_type(RowVersion)]
        .into_iter()
        .find(|names| !names.is_empty());
    let Some(names) = names else {
        if !len.is_null() {
            // SAFETY: not null, so writable, as # Safety says.
            unsafe { *len = 0 };
        }
        return fail(crate::AE_NOT_FOUND);
    };
    let text: String = names.iter().map(|name| format!("{name};")).collect();
    // SAFETY: `buf` and `len` are as # Safety says.
    unsafe { crate::write_name(&ansi::from_str_lossy(&text), buf, len) }
}

/// `AdsAddCustomKey`: adds the current record's key to a custom index order
/// (one created with `ADS_CUSTOM`), which then keeps it current as the record
/// changes; a record with a key in the order already fails with 7037, as in
/// the original engine.
#[unsafe(no_mangle)]
pub extern "system" fn AdsAddCustomKey(index: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsAddCustomKey");
    let backend = api(index);
    if let Err(error) = backend.index_info(index) {
        return error_code(error);
    }
    code(backend.add_custom_key(index))
}

/// `AdsDeleteCustomKey`: removes the current record's key from a custom
/// index order; a record without one is left alone.
#[unsafe(no_mangle)]
pub extern "system" fn AdsDeleteCustomKey(index: AdsHandle) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsDeleteCustomKey");
    let backend = api(index);
    if let Err(error) = backend.index_info(index) {
        return error_code(error);
    }
    code(backend.delete_custom_key(index))
}

/// `AdsGetKeyFilter`: a filter that identifies the current record by the unique
/// index whose tag sorts first, as the original engine builds it. `values_table`
/// (a values-table file) is null here. `options` bit 1 is the OData format, bit 2
/// encodes the result as UTF-8 (otherwise ANSI). The filter is written NUL-
/// terminated with `*length` set to its length; too small a buffer fails with
/// `AE_INSUFFICIENT_BUFFER`, `*length` set to the length needed.
///
/// # Safety
///
/// `values_table` must be null or a NUL-terminated string; `filter` must point to
/// `*length` writable bytes; `length` must be valid for a `Unsigned32` read and
/// write.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetKeyFilter(
    table: AdsHandle,
    values_table: *mut Unsigned8,
    options: Unsigned32,
    filter: *mut Unsigned8,
    length: *mut Unsigned32,
) -> Unsigned32 {
    let _call = crate::last_error::Call::begin("AdsGetKeyFilter");
    if filter.is_null() || length.is_null() {
        return fail(AE_INVALID_POINTER);
    }
    // SAFETY: `values_table` is null or NUL-terminated, as # Safety says.
    let values = unsafe { text(values_table) }.unwrap_or_default();
    let text = match api(table).key_filter(table, values, options) {
        Ok(text) => text,
        Err(error) => return error_code(error),
    };
    // Bit 2 (ADS_FILTER_ENCODE_UTF8) keeps the text UTF-8; otherwise ANSI.
    let bytes = if options & 0x2 != 0 {
        text.into_bytes()
    } else {
        ansi::data_out(&text)
    };
    // SAFETY: neither is null, and `filter` holds `*length` bytes, as # Safety
    // says.
    unsafe { write_string(&bytes, filter, length) }
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use super::*;
    use crate::*;

    /// The arguments `AdsSeek` checks, and `AdsIsFound`, as in the original
    /// engine; what seeks find is tested in the engine.
    #[test]
    fn seek_arguments() {
        // Seeks parse dates in the process-wide date format.
        let _settings = fixtures::settings_lock();
        use crate::{
            ADS_DOUBLEKEY, ADS_HARDSEEK, ADS_RAWKEY, ADS_SEEKGT, ADS_SOFTSEEK, ADS_STRINGKEY,
            ADS_WSTRINGKEY, AE_INVALID_DATE, AE_INVALID_HANDLE, AE_INVALID_KEY_LENGTH,
            AE_INVALID_OPTION, AdsGotoRecord, AdsIsFound, AdsSkip,
        };
        let (connect, table) = fixtures::open("adi_keys", c"keys.adt");
        let order = |name: &CStr| {
            let mut handle = 0;
            let rc =
                unsafe { AdsGetIndexHandle(table, name.as_ptr().cast_mut().cast(), &mut handle) };
            assert_eq!(rc, AE_SUCCESS);
            handle
        };
        let (chars, dates) = (order(c"I_CHAR"), order(c"I_DATE"));
        let seek = |index, key: &[u8], data_type, seek_type| {
            let mut found = 9;
            let key_ptr = key.as_ptr().cast_mut();
            let len = key.len() as u16;
            let rc = unsafe { AdsSeek(index, key_ptr, len, data_type, seek_type, &mut found) };
            (rc, found)
        };
        let is_found = |obj| {
            let mut found = 9;
            let rc = unsafe { AdsIsFound(obj, &mut found) };
            (rc, found)
        };

        assert_eq!(
            seek(chars, b"abc", ADS_STRINGKEY, ADS_SOFTSEEK),
            (AE_SUCCESS, ADS_TRUE)
        );
        assert_eq!(is_found(table), (AE_SUCCESS, ADS_TRUE));
        assert_eq!(
            is_found(chars),
            (AE_SUCCESS, ADS_TRUE),
            "asked on the order"
        );
        // A failed seek leaves the flag; a move clears it.
        assert_eq!(
            seek(chars, b"abc", 3, ADS_SOFTSEEK),
            (AE_INVALID_OPTION, ADS_FALSE)
        );
        assert_eq!(
            seek(chars, b"abc", ADS_STRINGKEY, 3),
            (AE_INVALID_OPTION, ADS_FALSE)
        );
        assert_eq!(
            seek(dates, b"13/01/2000", ADS_STRINGKEY, ADS_HARDSEEK),
            (AE_INVALID_DATE, 0)
        );
        assert_eq!(is_found(table), (AE_SUCCESS, ADS_TRUE));
        assert_eq!(AdsSkip(chars, 0), AE_SUCCESS);
        assert_eq!(is_found(table), (AE_SUCCESS, ADS_FALSE));

        let wide: Vec<u8> = "abc".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(
            seek(chars, &wide, ADS_WSTRINGKEY, ADS_HARDSEEK),
            (AE_SUCCESS, ADS_TRUE)
        );
        assert_eq!(
            seek(chars, b"abd", ADS_RAWKEY, ADS_HARDSEEK),
            (AE_SUCCESS, ADS_FALSE)
        );
        assert_eq!(
            seek(chars, b"abc", ADS_RAWKEY, ADS_SEEKGT),
            (AE_SUCCESS, ADS_TRUE)
        );
        let julian = 2458066f64.to_le_bytes();
        assert_eq!(
            seek(dates, &julian, ADS_DOUBLEKEY, ADS_SOFTSEEK),
            (AE_SUCCESS, ADS_TRUE)
        );
        assert_eq!(
            seek(dates, &julian[..4], ADS_DOUBLEKEY, ADS_SOFTSEEK),
            (AE_INVALID_KEY_LENGTH, 0)
        );

        // Handles and pointers.
        assert_eq!(
            seek(table, b"abc", ADS_STRINGKEY, ADS_SOFTSEEK),
            (AE_INVALID_INDEX_HANDLE, 9)
        );
        let rc = unsafe {
            AdsSeek(
                chars,
                c"abc".as_ptr().cast_mut().cast(),
                3,
                ADS_STRINGKEY,
                1,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(rc, AE_INVALID_POINTER);
        let mut found = 9;
        let null = std::ptr::null_mut();
        let rc = unsafe { AdsSeek(chars, null, 3, ADS_STRINGKEY, ADS_SOFTSEEK, &mut found) };
        assert_eq!((rc, found), (AE_INVALID_POINTER, ADS_FALSE));
        // An empty key finds the first key.
        assert_eq!(AdsGotoRecord(table, 3), AE_SUCCESS);
        let rc = unsafe { AdsSeek(chars, null, 0, ADS_STRINGKEY, ADS_SOFTSEEK, &mut found) };
        assert_eq!((rc, found), (AE_SUCCESS, ADS_TRUE));
        assert_eq!(is_found(connect), (AE_INVALID_HANDLE, 9));
        assert_eq!(
            unsafe { AdsIsFound(table, std::ptr::null_mut()) },
            AE_INVALID_POINTER
        );
        assert_eq!(crate::AdsDisconnect(connect), crate::AE_SUCCESS);
    }

    /// Where the table is: the record, or `E` at EOF, `B` at BOF.
    fn landed(table: AdsHandle) -> String {
        let (mut eof, mut bof) = (0, 0);
        unsafe { crate::AdsAtEOF(table, &mut eof) };
        unsafe { crate::AdsAtBOF(table, &mut bof) };
        match (bof != 0, eof != 0) {
            (true, true) => "BE".to_owned(),
            (false, true) => "E".to_owned(),
            (true, false) => "B".to_owned(),
            _ => {
                let mut record = 0;
                unsafe { crate::AdsGetRecordNum(table, ADS_IGNOREFILTERS, &mut record) };
                record.to_string()
            }
        }
    }

    /// Seeks, seek-lasts and scopes in the OEM `co` tables of
    /// `charset_default` (NTX and CDX), with the client's ANSI text, land
    /// where the original engine's do (`fixtures/seek/oem.json`, from
    /// `tools/ace/seek`): the text is converted to the OEM code page.
    #[test]
    fn oem_seeks_match_the_engine() {
        let json = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/seek/oem.json"),
        )
        .unwrap();
        let cases: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        let (mut connect, mut table) = (0, 0);
        let mut differences = Vec::new();
        for case in &cases {
            let line = case["case"].as_str().unwrap();
            let words: Vec<&str> = line.split(' ').collect();
            let unhex = |hex: &str| -> Vec<u8> {
                (0..hex.len() / 2 * usize::from(hex != "-"))
                    .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
                    .collect()
            };
            if words[0] == "open" {
                if connect != 0 {
                    crate::AdsDisconnect(connect);
                }
                let (kind, file) = words[3].split_once('/').unwrap();
                connect = fixtures::connect(&format!("{kind}/charset_default"));
                let table_type = if kind == "ntx" { ADS_NTX } else { ADS_CDX };
                let file = std::ffi::CString::new(file).unwrap();
                let null = std::ptr::null_mut();
                let rc = unsafe {
                    AdsOpenTable(
                        connect,
                        file.as_ptr().cast_mut().cast(),
                        null,
                        table_type,
                        ADS_OEM,
                        1,
                        1,
                        0,
                        &mut table,
                    )
                };
                assert_eq!(rc, AE_SUCCESS);
                for index in &words[4..] {
                    let index = std::ffi::CString::new(index.split_once('/').unwrap().1).unwrap();
                    let (mut handles, mut n) = ([0; 8], 8);
                    let index = index.as_ptr().cast_mut().cast();
                    let rc = unsafe { AdsOpenIndex(table, index, handles.as_mut_ptr(), &mut n) };
                    assert_eq!(rc, AE_SUCCESS);
                }
                continue;
            }
            let name = std::ffi::CString::new(words[1]).unwrap();
            let mut order = 0;
            unsafe { AdsGetIndexHandle(table, name.as_ptr().cast_mut().cast(), &mut order) };
            let mut found = 0;
            let got = match words[0] {
                "seek" | "last" => {
                    let mut key = unhex(words[words.len() - 1]);
                    let (ptr, len) = (key.as_mut_ptr(), key.len() as u16);
                    crate::AdsGotoRecord(table, 3);
                    let rc = unsafe {
                        match (words[0], words[2]) {
                            ("last", _) => AdsSeekLast(order, ptr, len, ADS_STRINGKEY, &mut found),
                            (_, how) => {
                                let how = match how {
                                    "soft" => ADS_SOFTSEEK,
                                    "hard" => ADS_HARDSEEK,
                                    _ => ADS_SEEKGT,
                                };
                                AdsSeek(order, ptr, len, ADS_STRINGKEY, how, &mut found)
                            }
                        }
                    };
                    assert_eq!(rc, AE_SUCCESS, "{line}");
                    landed(table) + if found != 0 { "*" } else { "" }
                }
                "scope" => {
                    let mut key = unhex(words[3]);
                    let option = if words[2] == "top" {
                        ADS_TOP
                    } else {
                        ADS_BOTTOM
                    };
                    let (ptr, len) = (key.as_mut_ptr(), key.len() as u16);
                    unsafe { AdsSetScope(order, option, ptr, len, ADS_STRINGKEY) }.to_string()
                }
                "clear" => {
                    let option = if words[2] == "top" {
                        ADS_TOP
                    } else {
                        ADS_BOTTOM
                    };
                    AdsClearScope(order, option).to_string()
                }
                "walk" => {
                    let mut forward = Vec::new();
                    crate::AdsGotoTop(order);
                    loop {
                        forward.push(landed(table));
                        if forward.last().unwrap().contains('E') {
                            break;
                        }
                        crate::AdsSkip(order, 1);
                    }
                    let mut back = Vec::new();
                    crate::AdsGotoBottom(order);
                    loop {
                        back.push(landed(table));
                        if back.last().unwrap().contains('B') {
                            break;
                        }
                        crate::AdsSkip(order, -1);
                    }
                    format!("{} / {}", forward.join(" "), back.join(" "))
                }
                other => panic!("unknown command {other}"),
            };
            if got != case["out"] {
                differences.push(format!("{line}: {got} / {}", case["out"]));
            }
        }
        crate::AdsDisconnect(connect);
        let count = differences.len();
        assert!(
            differences.is_empty(),
            "{count} differences:\n{}",
            differences.join("\n")
        );
    }

    /// Text sought in an NChar order is taken as Unicode, from UTF-16 as it
    /// is and from ANSI in the client's code page, and found by its ICU sort
    /// key, as in the original engine (`fixtures/seek/nchar.json`).
    #[test]
    fn nchar_seeks() {
        let (connect, table) = fixtures::open("adi_keys", c"keys.adt");
        let mut order = 0;
        let name = c"I_NCHAR".as_ptr().cast_mut().cast();
        assert_eq!(
            unsafe { AdsGetIndexHandle(table, name, &mut order) },
            AE_SUCCESS
        );
        let seek = |mut key: Vec<u8>, data_type, how| {
            let mut found = 0;
            let (ptr, len) = (key.as_mut_ptr(), key.len() as u16);
            let rc = unsafe { AdsSeek(order, ptr, len, data_type, how, &mut found) };
            assert_eq!(rc, AE_SUCCESS);
            format!("{}{}", landed(table), if found != 0 { "*" } else { "" })
        };
        let wide = |text: &str| {
            text.encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<u8>>()
        };
        assert_eq!(seek(wide("Ωж€"), ADS_WSTRINGKEY, ADS_HARDSEEK), "4*");
        assert_eq!(seek(wide("Ωж"), ADS_WSTRINGKEY, ADS_SOFTSEEK), "4");
        assert_eq!(seek(wide("ω"), ADS_WSTRINGKEY, ADS_SEEKGT), "4*");
        assert_eq!(seek(b"abc".to_vec(), ADS_STRINGKEY, ADS_HARDSEEK), "1*");
        assert_eq!(seek(b"ab".to_vec(), ADS_STRINGKEY, ADS_SOFTSEEK), "1");
        assert_eq!(seek(vec![0xE4], ADS_STRINGKEY, ADS_SOFTSEEK), "1");
        assert_eq!(seek(b"Zz ".to_vec(), ADS_STRINGKEY, ADS_SEEKGT), "4*");
        assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
    }

    crate::fixtures::remote_twins!(seek_arguments, oem_seeks_match_the_engine, nchar_seeks);
}
