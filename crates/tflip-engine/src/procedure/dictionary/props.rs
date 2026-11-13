//! What the `sp_Modify…Property` procedures do with their `Property` and
//! `Value` arguments before the write: the property is a name the procedure
//! knows or a number, whose type decides how the value is read
//! (`fixtures/sql/spddprops*.json`).

use tflip_sql::SqlError;

use crate::{Error, ace_error};

/// The type of a property's value.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Ty {
    Text,
    Flag,
    U16,
    U32,
}

/// The properties the procedures read a value for, by their `ADS_DD_*`
/// number; any other number is refused as no property.
fn type_of(id: u32) -> Option<Ty> {
    Some(match id {
        1 | 3 => Ty::Text,
        5 | 103 | 104 | 106 | 107 | 113 | 120 | 122 | 125 | 132 | 203 | 214 | 220 | 222 | 223
        | 301 | 1105 => Ty::Flag,
        100
        | 101
        | 102
        | 105
        | 114..=118
        | 121
        | 127..=130
        | 200
        | 201
        | 202
        | 209
        | 211
        | 213
        | 221
        | 300
        | 302..=304
        | 311..=313
        | 700
        | 704
        | 800..=803
        | 805
        | 806
        | 1101
        | 1404
        | 1409
        | 65535 => Ty::Text,
        108 | 109 | 111 | 112 | 204 | 215 | 216 | 219 | 306..=308 => Ty::U16,
        126 | 131 | 217 | 218 | 309 => Ty::U32,
        _ => return None,
    })
}

/// An SQL engine error of its own.
fn error(state: &str, native: u32, message: &str) -> Error {
    Error::Sql(SqlError {
        state: state.to_owned(),
        native,
        message: message.to_owned(),
    })
}

/// 5126 without its own text, as the procedure words it.
pub(super) fn not_a_property() -> Error {
    ace_error::sql_bare(5126)
}

/// The property number `text` names for a procedure that knows `names`, or
/// as a number: its leading digits, and nothing a number does not start
/// with is one (`65535`, `ADS_DD_PROPERTY_NOT_AVAIL`).
pub(super) fn id_of(names: &[(&str, u32)], text: &str) -> u32 {
    let text = text.trim_end();
    if let Some((_, id)) = names
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(text))
    {
        return *id;
    }
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    match digits.parse::<u64>() {
        Ok(n) if !digits.is_empty() => u32::try_from(n).unwrap_or(u32::MAX),
        _ => 65535,
    }
}

/// The bytes of `value` as the property `id` takes it, or the refusal of a
/// value that is none; for a property no procedure reads, `not_a_property`.
pub(super) fn convert(id: u32, value: Option<&str>) -> Result<(Ty, Vec<u8>), Error> {
    let ty = type_of(id).ok_or_else(not_a_property)?;
    let text = value.unwrap_or("");
    let bytes = match ty {
        Ty::Text => {
            let mut bytes = text.as_bytes().to_vec();
            bytes.push(0);
            bytes
        }
        Ty::Flag => {
            // The first character says: T, Y, 1 or F, N, 0.
            let on = match text.trim_start().chars().next() {
                // No bytes at all: what the write underneath makes of it is its own.
                None => return Ok((ty, Vec::new())),
                Some('1' | 'T' | 't' | 'Y' | 'y') => true,
                Some('0' | 'F' | 'f' | 'N' | 'n') => false,
                Some(_) => {
                    return Err(ace_error::sql_bare(5101));
                }
            };
            u16::from(on).to_le_bytes().to_vec()
        }
        Ty::U16 | Ty::U32 => {
            // No bytes at all, as with a flag: the write says what size it wanted.
            if text.trim().is_empty() {
                return Ok((ty, Vec::new()));
            }
            let n = text
                .trim()
                .parse::<i128>()
                .map_err(|_| error("07006", 2109, "Conversion error"))?;
            match ty {
                Ty::U16 => match u16::try_from(n) {
                    Ok(n) => n.to_le_bytes().to_vec(),
                    Err(_) => return Err(error("22003", 2111, "Out of range")),
                },
                _ => match u32::try_from(n) {
                    Ok(n) => n.to_le_bytes().to_vec(),
                    Err(_) => return Err(error("22003", 2111, "Out of range")),
                },
            }
        }
    };
    Ok((ty, bytes))
}
