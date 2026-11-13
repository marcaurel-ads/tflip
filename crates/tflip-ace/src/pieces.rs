//! Binary values set in pieces: `AdsSetBinary` with a total length and an
//! offset (`fixtures/write/binary_pieces`, probed with `tools/ace/write`).
//!
//! A piece goes into the field's value in the current record, cut or padded
//! with zeros to the total: a total of 10 with 5 bytes at offset 0 keeps
//! the 5 bytes after them the field held, or zeros where it held none.
//!
//! - A piece past the total, or longer than it, fails with
//!   `AE_INVALID_BINARY_OFFSET` before the record is looked at; a total and
//!   a length of 0 is NULL; a length of 0 leaves the value as it is, cut or
//!   padded to the total.
//! - A compressed field takes no pieces (5227), only the whole value.
//! - An Image field takes `ADS_IMAGE` and a Binary field `ADS_BINARY`
//!   (5066 for the other), any other type is 5070.
//!
//! Not as in the original engine: it keeps the bytes of the value it cut
//! (a value of 10 bytes cut to 4 and set to a total of 10 again has its old
//! bytes 5 to 10 back, even after a write), where tflip pads with zeros
//! (DIFFERENCES.md).

use tflip_engine::getters::{FieldId, Scalar, Want};
use tflip_engine::handle::HandleKind;
use tflip_engine::setters::SetValue;
use tflip_engine::table::TableType;
use tflip_storage::field::FieldType;

use crate::backend::{Backend, api};
use crate::field::{FieldArg, with_field};
use crate::last_error::{fail_detail, fail_text};
use crate::setters::set;
use crate::{
    ADS_BINARY, ADS_IMAGE, AE_INVALID_BINARY_OFFSET, AE_INVALID_DATA_TYPE, AE_INVALID_FIELD_TYPE,
    AdsHandle, Unsigned16, Unsigned32,
};

/// `AE_COMPRESS_ERROR`, the code of a compressed field set in pieces.
const AE_COMPRESS_ERROR: Unsigned32 = 5227;

/// The part of a value an `AdsSetBinary` call gives.
pub(crate) struct Piece {
    pub binary_type: Unsigned16,
    pub total: Unsigned32,
    pub offset: Unsigned32,
}

/// The value of field `number` in the current record, empty for NULL.
fn value_of(backend: &Backend, handle: AdsHandle, number: usize) -> Option<Vec<u8>> {
    match backend.field_as(
        handle,
        FieldId::Number(number),
        Want::Field {
            utf8: false,
            ansi_dates: false,
        },
    ) {
        Ok(Scalar::FieldText {
            text: tflip_engine::format::FieldText::Bytes(bytes),
            ..
        }) => Some(bytes),
        _ => None,
    }
}

/// Sets a Binary, Image or VarBinary field of a table or cursor to `piece`,
/// `bytes`, written into the field's bytes as above; `None` for any other
/// handle or field, which `AdsSetBinary` sets as a whole.
pub(crate) fn set_binary(
    handle: AdsHandle,
    field: &FieldArg,
    piece: Piece,
    bytes: &[u8],
) -> Option<Unsigned32> {
    let backend = api(handle);
    if !matches!(
        backend.handle_kind(handle),
        Ok(Some(HandleKind::Table | HandleKind::Cursor))
    ) {
        return None;
    }
    // DBF tables are not probed.
    let adt = backend
        .table_info(handle)
        .is_ok_and(|info| info.settings.table_type == TableType::Adt);
    if !adt {
        return None;
    }
    if piece.binary_type != ADS_BINARY && piece.binary_type != ADS_IMAGE {
        return Some(fail_detail(AE_INVALID_DATA_TYPE, "Parameter 3"));
    }
    let (name, field_type, compressed) = with_field(handle, field.clone(), |f| {
        (f.name.clone(), f.field_type, f.compressed())
    })
    .ok()?;
    let wanted = if piece.binary_type == ADS_IMAGE {
        FieldType::Image
    } else {
        FieldType::Binary
    };
    let memo = matches!(field_type, FieldType::Binary | FieldType::Image);
    if memo && field_type != wanted {
        return Some(fail_detail(
            AE_INVALID_FIELD_TYPE,
            &format!("Field: \"{name}\""),
        ));
    }
    // The others fail as the whole value does.
    if !memo && !(field_type == FieldType::VarBinary && wanted == FieldType::Binary) {
        return None;
    }
    let len = bytes.len() as u64;
    if len > u64::from(piece.total) || u64::from(piece.offset) + len > u64::from(piece.total) {
        return Some(fail_detail(
            AE_INVALID_BINARY_OFFSET,
            &format!("Field: \"{name}\""),
        ));
    }
    let whole = |bytes: Vec<u8>| match wanted {
        FieldType::Image => SetValue::Image(bytes),
        _ => SetValue::Binary(bytes),
    };
    let number = field.number(&backend, handle).ok()?;
    let current = backend.record_number(handle, false).unwrap_or(0) != 0;
    if compressed && current && len != u64::from(piece.total) {
        return Some(fail_text(
            AE_COMPRESS_ERROR,
            format!(
                "Error {AE_COMPRESS_ERROR}:  {}  Compressible memo and blob field type cannot be update in chunks. Field: \"{name}\"",
                crate::last_error::error_text(AE_COMPRESS_ERROR).unwrap_or_default()
            ),
        ));
    }
    // The whole value, with no record to set it in, and one too large to
    // pad are the whole setter's.
    if !current || piece.total > 0x4000_0000 || (piece.offset == 0 && len == u64::from(piece.total))
    {
        return None;
    }
    if piece.total == 0 {
        return Some(set(
            "AdsSetBinary",
            handle,
            field.clone(),
            SetValue::Null,
            None,
        ));
    }
    let mut content = value_of(&backend, handle, number).unwrap_or_default();
    content.resize(piece.total as usize, 0);
    let start = piece.offset as usize;
    content[start..start + bytes.len()].copy_from_slice(bytes);
    Some(set(
        "AdsSetBinary",
        handle,
        field.clone(),
        whole(content),
        None,
    ))
}
