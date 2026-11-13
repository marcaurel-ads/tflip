//! ADI sort keys: the bytes an order stores for a value. Comparing keys as
//! unsigned bytes gives index order. A NULL value's key is all zero bytes,
//! which sorts first; a multi-field key is the concatenation of its fields'
//! keys.

use crate::cp1252;
use crate::field::{Field, FieldType};
use crate::value::{Value, scaled_decimal};

const SIGN: u64 = 1 << 63;

/// The big-endian bytes of `n` with the sign bit flipped, which sort as
/// the numbers do.
pub fn sortable(n: i64, bytes: usize) -> Vec<u8> {
    let flipped = (n as u64) ^ (1 << (bytes * 8 - 1));
    flipped.to_be_bytes()[8 - bytes..].to_vec()
}

/// An IEEE double as an order-preserving key: big-endian, with the sign bit
/// set for positive values and zero, and all bits inverted for negative
/// values. The key of the integer types (but LongInt and RowVersion), of
/// Double, CurDouble and Date (the Julian day).
pub fn double(value: f64) -> [u8; 8] {
    let bits = value.to_bits();
    let bits = if value < 0.0 { !bits } else { bits | SIGN };
    bits.to_be_bytes()
}

/// A 64-bit integer as a key: big-endian with the sign bit flipped. The key
/// of LongInt, RowVersion and Money (the value × 10,000).
pub fn int64(value: i64) -> [u8; 8] {
    (value as u64 ^ SIGN).to_be_bytes()
}

/// The key of a Numeric(`length`, `decimals`) value given as its value ×
/// 10^`decimals`: packed decimal of *p* nibbles, *p* being `length` less one
/// for the decimal point if there is one. A sign nibble (8 positive, 4
/// negative) comes first, then *p* − 1 digits, in nines' complement if
/// negative; when *p* is odd, a 0xF nibble fills the last byte. `None` if the
/// value has more digits than that.
pub fn numeric(scaled: i128, length: u16, decimals: u16) -> Option<Vec<u8>> {
    let p = usize::from(if decimals > 0 {
        length.saturating_sub(1)
    } else {
        length
    });
    let digits = p.checked_sub(1)?;
    let text = scaled.unsigned_abs().to_string();
    if text.len() > digits {
        return None;
    }
    let negative = scaled < 0;
    let mut nibbles = vec![if negative { 4 } else { 8 }];
    nibbles.extend(
        std::iter::repeat_n(b'0', digits - text.len())
            .chain(text.bytes())
            .map(|d| {
                let d = d - b'0';
                if negative { 9 - d } else { d }
            }),
    );
    if p % 2 == 1 {
        nibbles.push(0xF);
    }
    Some(
        nibbles
            .chunks(2)
            .map(|pair| pair[0] << 4 | pair[1])
            .collect(),
    )
}

/// The key of a Time: milliseconds since midnight plus one, big-endian.
pub fn time(milliseconds: u32) -> [u8; 4] {
    milliseconds.wrapping_add(1).to_be_bytes()
}

/// The key of a TimeStamp: the Julian day, then the time's key.
pub fn timestamp(julian: i32, milliseconds: u32) -> [u8; 8] {
    let mut key = [0; 8];
    key[..4].copy_from_slice(&(julian as u32).to_be_bytes());
    key[4..].copy_from_slice(&time(milliseconds));
    key
}

/// The day number a CDX order's key holds for a Visual FoxPro TimeStamp
/// (its key is [`double`] of it), as the original engine computes it
/// (`fixtures/write/vfp_timestamps`): the time rounded to the nearest
/// second, the day and the seconds as seconds, times the double nearest
/// 1/86400 (so a quarter of the values are one bit below the double nearest
/// the day and its fraction). A time that rounds up to midnight gives the
/// next day; if that day's double has no bits in its low 32 (and the day
/// is not 0), the original takes one off its high 32 bits (`ADS_BUGS.md`),
/// so the key sorts a little before that day's midnight.
pub fn vfp_timestamp(julian: i32, milliseconds: u32) -> f64 {
    let seconds = (u64::from(milliseconds) + 500) / 1000;
    let day = f64::from(julian) * 86_400.0;
    let key = (day + seconds as f64) * (1.0 / 86_400.0);
    if seconds < 86_400 || julian == 0 {
        return key;
    }
    let bits = key.to_bits();
    if bits & 0xFFFF_FFFF == 0 {
        f64::from_bits(bits - (1 << 32))
    } else {
        key
    }
}

/// The key of a Logical: `T` or `F`.
pub fn logical(value: bool) -> [u8; 1] {
    [if value { b'T' } else { b'F' }]
}

/// The key of a character value under the default collation: its bytes,
/// padded with spaces (or cut) to `key_length`.
pub fn text(bytes: &[u8], key_length: usize) -> Vec<u8> {
    let mut key = bytes[..bytes.len().min(key_length)].to_vec();
    key.resize(key_length, b' ');
    key
}

/// The length of `field`'s keys in an order on several fields (`a;b`), whose
/// keys are its fields' keys one after the other: the declared length for
/// the character and Raw types, the packed decimal's for Numeric (see
/// [`numeric`]), 3 a character and 6 for NChar and NVarChar, 1 for Logical,
/// 4 for Time, 16 for GUID, and 8 for the others.
pub fn field_key_length(field: &Field) -> usize {
    field_key_length_wide(field, 0)
}

/// [`field_key_length`] in an order whose Unicode keys take `width` bytes a
/// character (the tag's [`crate::order::Tag::uchar_width`]: 2, 4 or 5 as
/// `ADS_UCHAR_KEY_SHORT`, `_LONG` and `_XLONG` ask for, else 3).
pub fn field_key_length_wide(field: &Field, width: u8) -> usize {
    use FieldType::*;
    match field.field_type {
        Character | CiCharacter | VarChar | LegacyVarChar | Raw => usize::from(field.length()),
        Numeric => {
            let p = usize::from(field.length()) - usize::from(field.decimals() > 0);
            p.div_ceil(2)
        }
        // Three bytes a character and six more (probed on
        // `fixtures/adt/adi_keys` and `fixtures/write/nchar_indexes`).
        NChar | NVarChar => unicode_width(width) * usize::from(field.length()) + 6,
        Logical => 1,
        Time => 4,
        Guid => 16,
        _ => 8,
    }
}

/// The bytes a character takes in the Unicode keys of an order whose tag has
/// `width` (see [`crate::order::locale::WIDTH`]).
pub fn unicode_width(width: u8) -> usize {
    match width {
        2 | 4 | 5 => usize::from(width),
        _ => 3,
    }
}

/// The key of `value`, the value of `field`, in an order on that field alone
/// whose keys are `key_length` bytes. Character keys are the default
/// collation's (the bytes themselves); the static and dynamic collations,
/// whose keys are weights, are not supported. NChar and NVarChar keys are
/// ICU sort keys (see [`crate::icu::nchar_key`]). `None` for a type whose
/// keys are not known, text with a character whose ICU weights tflip does
/// not know, and a value that is not of the field's type.
pub fn field_key(field: &Field, value: &Value, key_length: usize) -> Option<Vec<u8>> {
    use FieldType::*;
    let key = match (field.field_type, value) {
        (_, Value::Null) => vec![0; key_length],
        (Character | VarChar, Value::Text(t)) => text(&cp1252::encode(t), key_length),
        (CiCharacter, Value::Text(t)) => text(&cp1252::encode(t).to_ascii_lowercase(), key_length),
        (NChar | NVarChar, Value::Text(t)) => crate::icu::nchar_key(t, key_length)?,
        (Numeric, Value::Numeric(t)) => {
            let scaled = scaled_decimal(t, u32::from(field.decimals()))?;
            numeric(scaled, field.length(), field.decimals())?
        }
        (LongInt | RowVersion, Value::Integer(i)) => int64(*i as i64).to_vec(),
        (ShortInt | Integer | AutoInc, Value::Integer(i)) => double(*i as f64).to_vec(),
        (Double | CurDouble, Value::Double(d)) => double(*d).to_vec(),
        (Money, Value::Money(m)) => int64(*m).to_vec(),
        (Date, Value::Date(julian)) => double(f64::from(*julian)).to_vec(),
        (Time, Value::Time(ms)) => time(*ms).to_vec(),
        (TimeStamp | ModTime, Value::TimeStamp(julian, ms)) => timestamp(*julian, *ms).to_vec(),
        (Logical, Value::Logical(b)) => logical(*b).to_vec(),
        (Raw, Value::Binary(bytes)) => bytes.clone(),
        (Guid, Value::Guid(bytes)) => bytes.to_vec(),
        _ => return None,
    };
    (key.len() == key_length).then_some(key)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use serde_json::Value as Json;

    use super::*;
    use crate::adi;
    use crate::table::{Format, TableFile};

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn encodings() {
        assert_eq!(hex(&double(1.0)), "bff0000000000000");
        assert_eq!(hex(&double(-1.0)), "400fffffffffffff");
        assert_eq!(hex(&double(0.0)), "8000000000000000");
        assert_eq!(hex(&double(-0.0)), "8000000000000000");
        assert_eq!(hex(&int64(-1)), "7fffffffffffffff");
        assert_eq!(hex(&int64(1)), "8000000000000001");
        // Numeric(8, 2): seven nibbles, filled to four bytes.
        assert_eq!(hex(&numeric(150, 8, 2).unwrap()), "8000150f");
        assert_eq!(hex(&numeric(-150, 8, 2).unwrap()), "4999849f");
        assert_eq!(hex(&numeric(999_999, 8, 2).unwrap()), "8999999f");
        assert_eq!(numeric(1_000_000, 8, 2), None);
        // Numeric(4, 0): four nibbles.
        assert_eq!(hex(&numeric(-12, 4, 0).unwrap()), "4987");
        assert_eq!(hex(&time(0)), "00000001");
        assert_eq!(hex(&timestamp(1721426, 0)), "001a445200000001");
        assert_eq!(text(b"ab", 4), b"ab  ");
        assert_eq!(text(b"abcdef", 4), b"abcd");
    }

    /// Every key of every order on a single field in the fixtures, rebuilt
    /// from the field's value in the key's record.
    #[test]
    fn field_keys_match_the_engine() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt");
        let mut checked = 0;
        for dir in std::fs::read_dir(fixtures).unwrap() {
            for entry in std::fs::read_dir(dir.unwrap().path()).unwrap() {
                let path: PathBuf = entry.unwrap().path();
                if path.extension().is_some_and(|e| e == "adt") {
                    checked += check_table(&path);
                }
            }
        }
        assert!(checked >= 30, "checked only {checked} orders");
    }

    /// Checks the keys of `path`'s single-field orders; returns how many
    /// orders it checked.
    fn check_table(path: &Path) -> usize {
        let json = std::fs::read_to_string(path.with_extension("json")).unwrap();
        let dump: Json = serde_json::from_str(&json).unwrap();
        let Ok(mut table) = TableFile::open(path, Format::Adt) else {
            return 0;
        };
        let mut checked = 0;
        for order in dump["indexes"].as_array().into_iter().flatten() {
            let Some(keys) = order["keys"].as_array() else {
                continue;
            };
            let file = path.with_file_name(order["file"].as_str().unwrap());
            let Ok(index) = adi::IndexFile::open(&file) else {
                continue;
            };
            let name = order["name"].as_str().unwrap();
            let tag = index.tags.iter().find(|t| t.name == name).unwrap();
            // An order on one field, with the default collation.
            let Some(number) = tag
                .expression
                .strip_prefix('F')
                .and_then(|n| n.parse::<usize>().ok())
            else {
                continue;
            };
            let field = table.structure.fields[number - 1].clone();
            let field = &field;
            // Collations other than the default have their own weights.
            let collated =
                !tag.collation.is_empty() || dump["collation"] != "" || dump["ansi_char_set"] != "";
            let unicode = matches!(field.field_type, FieldType::NChar | FieldType::NVarChar);
            if collated || unicode || tag.binary() || tag.full_text() {
                continue;
            }
            let length = table.structure.header.record_length as usize;
            for key in keys {
                let record = key["recno"].as_u64().unwrap() as u32;
                let mut buf = vec![0; length];
                table.read(record, &mut buf).unwrap();
                let value = field.value(&buf, table.memo()).unwrap();
                let built = field_key(field, &value, usize::from(tag.key_length));
                let at = format!("{} {name} record {record}: {value:?}", path.display());
                assert_eq!(
                    built.as_deref().map(hex),
                    key["key"].as_str().map(str::to_lowercase),
                    "{at}"
                );
            }
            checked += 1;
        }
        checked
    }
}
