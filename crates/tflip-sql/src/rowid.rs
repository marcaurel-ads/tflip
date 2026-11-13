//! The `ROWID` of a record: 18 characters, the database, the table and the
//! record number as six base 64 digits each (`fixtures/sql/rowid.json`).
//!
//! The digits are `A-Z`, `a-z`, `0-9`, `+` and `/`, the most significant
//! first. The database's number is the CRC-32 of its path in upper case (the
//! folder of a free-table connection, the `.add` of a dictionary), as the
//! original engine writes paths: backslashes, a drive letter, no closing
//! backslash. A free table's is the CRC-32 of its file's path in upper case
//! (`DB\FOLDER\NAME.ADT`); a dictionary table's is its object id in the
//! dictionary, though the help says the file name decides it.

const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The table of a record, as a `ROWID` names it.
#[derive(Debug, Clone, Copy)]
pub enum Table<'a> {
    /// A free table, by its file's folder and name.
    File { folder: &'a str, name: &'a str },
    /// A table of the connection's dictionary, by its object id.
    Object(u32),
}

fn digits(n: u32) -> String {
    (0..6)
        .rev()
        .map(|i| DIGITS[(n >> (6 * i) & 63) as usize] as char)
        .collect()
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// A path as the original engine hashes it.
fn path(text: &str) -> String {
    text.replace('/', "\\")
        .trim_end_matches('\\')
        .to_uppercase()
}

/// The first twelve characters of the `ROWID`s of `table`'s records:
/// `database` is the connection's path (what `DATABASE()` gives).
pub fn prefix(database: &str, table: Table<'_>) -> String {
    let folder = path(database);
    let table = match table {
        Table::File { folder, name } => {
            crc32(format!("{}\\{}", path(folder), name.to_uppercase()).as_bytes())
        }
        Table::Object(id) => id,
    };
    format!("{}{}", digits(crc32(folder.as_bytes())), digits(table))
}

/// The `ROWID` of record `record` of the table `prefix` is of.
pub fn encode(prefix: &str, record: u32) -> String {
    format!("{prefix}{}", digits(record))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_is_ieee() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn records_count_in_base_64() {
        assert_eq!(digits(1), "AAAAAB");
        assert_eq!(digits(31), "AAAAAf");
        assert_eq!(digits(63), "AAAAA/");
        assert_eq!(digits(64), "AAAABA");
        assert_eq!(digits(128), "AAAACA");
    }
}
