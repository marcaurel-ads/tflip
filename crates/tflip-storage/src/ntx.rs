//! Clipper indexes (`.ntx`): one index order per file, a B-tree of
//! 1,024-byte pages with uncompressed keys.
//!
//! The header (the first page) holds the root page, the key length, the
//! key expression, the `FOR` condition, and whether the order is unique and
//! descending. A page is a u16 key count *n*, the offsets of its *n* + 1
//! items, and the items: a u32 child page offset (0 in a leaf), a u32
//! record number and the key. Item *n* holds only the rightmost child. The
//! tree is a B-tree, not a B+ tree: interior pages hold keys too, so an
//! in-order walk visits item 0's child, key 0, item 1's child, key 1, …,
//! then item *n*'s child.
//!
//! Keys are stored in index order, descending ones too, and equal keys in
//! the order of their records. They are ordered by the weights of the
//! table's static collation, which are the bytes themselves under the
//! default one.

use crate::bytes::text;
use std::cmp::Ordering;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::order::{Error, Key};

/// The size of the header and of a page.
pub(crate) const PAGE_SIZE: usize = 1024;

/// The deepest tree the original engine walks.
pub(crate) const MAX_DEPTH: usize = 30;

/// The key of the number `value` in an order whose keys are `length`
/// characters with `decimals` decimals: zero-padded, rounded half away from
/// zero (`00001.50`); a negative number's digits stored as 0x5C minus
/// themselves (-1.5 is `,,,,+.',`), so that it sorts before the positive
/// ones; asterisks if it does not fit.
pub fn number_key(value: f64, length: usize, decimals: usize) -> Vec<u8> {
    // The exact decimal expansion, rounded at the decimals.
    let exact = format!("{:.*}", decimals + 30, value.abs());
    let Some(scaled) = crate::value::scaled_decimal(&exact, decimals as u32) else {
        return vec![b'*'; length];
    };
    let negative = value < 0.0 && scaled != 0;
    let digits = format!("{scaled:0>width$}", width = decimals + 1);
    let (whole, fraction) = digits.split_at(digits.len() - decimals);
    let text = match decimals {
        0 => whole.to_owned(),
        _ => format!("{whole}.{fraction}"),
    };
    if text.len() > length {
        return vec![b'*'; length];
    }
    let mut key = format!("{text:0>length$}").into_bytes();
    if negative {
        for b in key.iter_mut().filter(|b| b.is_ascii_digit()) {
            *b = 0x5C - *b;
        }
    }
    key
}

/// The key of the date `julian`: `YYYYMMDD`, or spaces for 0 (an empty
/// date). A computed date past the year 9999 has `****` for its year
/// (`d+1` on 9999-12-31 is `****0101`, `fixtures/ntx/ntx_expr`).
pub fn date_key(julian: i32) -> Vec<u8> {
    if julian != 0 {
        let date = crate::value::julian_to_ymd(julian);
        if date.year > 9999 {
            return format!("****{:02}{:02}", date.month, date.day).into_bytes();
        }
    }
    crate::value::date_text(julian).to_vec()
}

/// What the header of an NTX file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// The byte offset of the root page.
    pub root: u32,
    pub key_length: u16,
    /// The decimals of a numeric key.
    pub decimals: u16,
    /// The key expression, as written (with field names).
    pub expression: String,
    /// The `FOR` condition, as written; empty if none.
    pub condition: String,
    pub unique: bool,
    pub descending: bool,
}

/// An open NTX file.
#[derive(Debug)]
pub struct NtxFile {
    pub(crate) file: File,
    pub header: Header,
    /// The weights keys are ordered by (the static collation's), if they
    /// are not the bytes themselves.
    weights: Option<Box<[u8; 256]>>,
}

/// A decoded page.
#[derive(Debug, Clone)]
struct Page {
    /// The child page left of each item (item `n` is the rightmost); 0 in a
    /// leaf.
    children: Vec<u32>,
    /// The keys of the first `n` items.
    keys: Vec<Key>,
}

impl NtxFile {
    /// Opens the NTX file at `path` and reads its header.
    pub fn open(path: &Path) -> Result<NtxFile, Error> {
        NtxFile::from_file(File::open(path).map_err(Error::Io)?)
    }

    /// Opens the NTX file at `path` to read and write (see
    /// [`NtxFile::insert_key`]).
    pub fn open_for_writing(path: &Path) -> Result<NtxFile, Error> {
        let file = File::options()
            .read(true)
            .write(true)
            .open(path)
            .map_err(Error::Io)?;
        NtxFile::from_file(file)
    }

    fn from_file(mut file: File) -> Result<NtxFile, Error> {
        let mut bytes = [0; PAGE_SIZE];
        file.read_exact(&mut bytes)
            .map_err(|_| Error::Corrupt("NTX file too short"))?;
        let u16_at = |offset: usize| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        let signature = u16_at(0);
        if !matches!(signature, 6 | 7) {
            return Err(Error::Corrupt("not an NTX file"));
        }
        let key_length = u16_at(0x0E);
        if key_length == 0 || usize::from(u16_at(0x0C)) != usize::from(key_length) + 8 {
            return Err(Error::Corrupt(
                "NTX item size does not match its key length",
            ));
        }
        let header = Header {
            root: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            key_length,
            decimals: u16_at(0x10),
            expression: text(&bytes[0x16..0x116]),
            condition: if signature == 7 {
                text(&bytes[0x11A..0x21A])
            } else {
                String::new()
            },
            unique: bytes[0x116] != 0,
            descending: bytes[0x118] != 0,
        };
        Ok(NtxFile {
            file,
            header,
            weights: None,
        })
    }

    /// Orders keys by `weights` (a static collation's), byte by byte.
    pub fn set_weights(&mut self, weights: Option<[u8; 256]>) {
        self.weights = weights
            .filter(|w| w.iter().enumerate().any(|(i, &b)| usize::from(b) != i))
            .map(Box::new);
    }

    /// Reads and decodes the page at byte offset `offset`.
    fn page(&mut self, offset: u32) -> Result<Page, Error> {
        if offset == 0 || !(offset as usize).is_multiple_of(PAGE_SIZE) {
            return Err(Error::Corrupt("NTX page offset not on a page"));
        }
        let mut bytes = [0; PAGE_SIZE];
        self.file.seek(SeekFrom::Start(u64::from(offset)))?;
        self.file
            .read_exact(&mut bytes)
            .map_err(|_| Error::Corrupt("NTX file ends inside a page"))?;
        let n = usize::from(u16::from_le_bytes([bytes[0], bytes[1]]));
        let key_length = usize::from(self.header.key_length);
        if 2 + 2 * (n + 1) > PAGE_SIZE {
            return Err(Error::Corrupt("NTX page holds too many keys"));
        }
        let mut children = Vec::with_capacity(n + 1);
        let mut keys = Vec::with_capacity(n);
        for i in 0..=n {
            let at = usize::from(u16::from_le_bytes([bytes[2 + 2 * i], bytes[3 + 2 * i]]));
            let end = at + 8 + if i < n { key_length } else { 0 };
            if at < 2 || end > PAGE_SIZE {
                return Err(Error::Corrupt("NTX item outside its page"));
            }
            children.push(u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()));
            if i < n {
                let record = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap());
                keys.push(Key {
                    bytes: bytes[at + 8..end].to_vec(),
                    record,
                });
            }
        }
        Ok(Page { children, keys })
    }

    /// How `key` compares with `sought` over `sought`'s length, by weight.
    pub(crate) fn compare(&self, key: &[u8], sought: &[u8]) -> Ordering {
        let length = sought.len().min(key.len());
        let (key, sought) = (&key[..length], &sought[..length]);
        crate::collation::weighed_cmp(key, sought, self.weights.as_deref())
    }

    /// How the entry `key` compares with `sought` and `record` in index
    /// order: by weight (reversed in a descending order), then by record.
    fn order(&self, key: &Key, sought: &[u8], record: Option<u32>) -> Ordering {
        let by_key = self.compare(&key.bytes, sought);
        let by_key = if self.header.descending {
            by_key.reverse()
        } else {
            by_key
        };
        by_key.then_with(|| match record {
            Some(record) if sought.len() >= key.bytes.len() => key.record.cmp(&record),
            _ => Ordering::Equal,
        })
    }

    /// The number of keys in the subtree whose root is the page at `offset`.
    fn subtree_keys(&mut self, offset: u32, depth: usize) -> Result<u32, Error> {
        if offset == 0 {
            return Ok(0);
        }
        if depth > MAX_DEPTH {
            return Err(Error::Corrupt("NTX tree too deep"));
        }
        let page = self.page(offset)?;
        let mut count = page.keys.len() as u32;
        for child in page.children {
            count += self.subtree_keys(child, depth + 1)?;
        }
        Ok(count)
    }
}

/// One level of a cursor's path: a page, and the item it is at (on its
/// key, or in the child left of it).
#[derive(Debug, Clone)]
struct Level {
    page: Page,
    item: usize,
}

/// A position in an NTX file's order, moving through its keys in index
/// order: before the first key, on a key, or after the last. It holds the
/// path from the root to the page it is on.
#[derive(Debug, Clone, Default)]
pub struct Cursor {
    path: Vec<Level>,
    /// After the last key (else before the first) when off a key.
    past_end: bool,
}

impl Cursor {
    /// A cursor before the first key.
    pub fn new() -> Cursor {
        Cursor::default()
    }

    /// The key the cursor is on, if any.
    pub fn key(&self) -> Option<&Key> {
        let level = self.path.last()?;
        level.page.keys.get(level.item)
    }

    pub fn before_first(&self) -> bool {
        self.path.is_empty() && !self.past_end
    }

    pub fn after_last(&self) -> bool {
        self.path.is_empty() && self.past_end
    }

    fn off(&mut self, past_end: bool) {
        self.path.clear();
        self.past_end = past_end;
    }

    /// Descends from the page at `offset` along its leftmost (or
    /// `rightmost`) children to a leaf, and stands on its first (or last)
    /// key; off the keys if the subtree has none.
    fn descend(
        &mut self,
        file: &mut NtxFile,
        mut offset: u32,
        rightmost: bool,
    ) -> Result<(), Error> {
        loop {
            if self.path.len() >= MAX_DEPTH {
                return Err(Error::Corrupt("NTX tree too deep"));
            }
            let page = file.page(offset)?;
            let item = if rightmost { page.keys.len() } else { 0 };
            let child = page.children[item];
            self.path.push(Level { page, item });
            if child == 0 {
                break;
            }
            offset = child;
        }
        let level = self.path.last_mut().unwrap();
        if rightmost {
            match level.item.checked_sub(1) {
                Some(item) => level.item = item,
                // An empty leaf: only an empty root is one.
                None => self.off(false),
            }
        } else if level.page.keys.is_empty() {
            self.off(true);
        }
        Ok(())
    }

    /// Moves to the first key, or after the last if there is none.
    pub fn first(&mut self, file: &mut NtxFile) -> Result<Option<&Key>, Error> {
        self.path.clear();
        let root = file.header.root;
        self.descend(file, root, false)?;
        if self.path.is_empty() {
            self.past_end = true;
        }
        Ok(self.key())
    }

    /// Moves to the last key, or before the first if there is none.
    pub fn last(&mut self, file: &mut NtxFile) -> Result<Option<&Key>, Error> {
        self.path.clear();
        let root = file.header.root;
        self.descend(file, root, true)?;
        Ok(self.key())
    }

    /// Moves to the next key, or after the last. From before the first key
    /// that is the first key; after the last it stays there.
    pub fn next(&mut self, file: &mut NtxFile) -> Result<Option<&Key>, Error> {
        if self.path.is_empty() {
            return match self.past_end {
                true => Ok(None),
                false => self.first(file),
            };
        }
        let level = self.path.last_mut().unwrap();
        level.item += 1;
        let right = level.page.children[level.item];
        if right != 0 {
            self.descend(file, right, false)?;
            return Ok(self.key());
        }
        // Up to the first level whose item is a key.
        while let Some(level) = self.path.last() {
            if level.item < level.page.keys.len() {
                return Ok(self.key());
            }
            self.path.pop();
        }
        self.off(true);
        Ok(None)
    }

    /// Moves to the previous key, or before the first. From after the last
    /// key that is the last key; before the first it stays there.
    pub fn prev(&mut self, file: &mut NtxFile) -> Result<Option<&Key>, Error> {
        if self.path.is_empty() {
            return match self.past_end {
                true => self.last(file),
                false => Ok(None),
            };
        }
        let level = self.path.last().unwrap();
        let left = level.page.children[level.item];
        if left != 0 {
            self.descend(file, left, true)?;
            return Ok(self.key());
        }
        // Up to the first level with a key before its item.
        while let Some(level) = self.path.last_mut() {
            if level.item > 0 {
                level.item -= 1;
                return Ok(self.key());
            }
            self.path.pop();
        }
        self.off(false);
        Ok(None)
    }

    /// Moves to the first key, in index order, that is not before `key`
    /// and `record`, or after the last key if there is none. Keys compare
    /// by their first `key.len()` bytes; with `record`, a full key equal in
    /// them compares by its record number.
    pub fn seek(
        &mut self,
        file: &mut NtxFile,
        key: &[u8],
        record: Option<u32>,
    ) -> Result<Option<&Key>, Error> {
        self.path.clear();
        let mut offset = file.header.root;
        loop {
            if self.path.len() >= MAX_DEPTH {
                return Err(Error::Corrupt("NTX tree too deep"));
            }
            let page = file.page(offset)?;
            let item = page
                .keys
                .partition_point(|entry| file.order(entry, key, record) == Ordering::Less);
            let child = page.children[item];
            self.path.push(Level { page, item });
            if child == 0 {
                break;
            }
            offset = child;
        }
        while let Some(level) = self.path.last() {
            if level.item < level.page.keys.len() {
                return Ok(self.key());
            }
            self.path.pop();
        }
        self.off(true);
        Ok(None)
    }

    /// The place of the current key in index order, 1-based; `None` off the
    /// keys. Counts the keys before it, reading the subtrees left of its
    /// path.
    pub fn key_number(&self, file: &mut NtxFile) -> Result<Option<u32>, Error> {
        if self.key().is_none() {
            return Ok(None);
        }
        // At each level, the keys before its item and the subtrees left of
        // them; at the last, also the subtree left of the current key.
        let mut before = 0;
        let last = self.path.len() - 1;
        for (depth, level) in self.path.iter().enumerate() {
            before += level.item as u32;
            let left = if depth == last {
                level.item + 1
            } else {
                level.item
            };
            for &child in &level.page.children[..left] {
                before += file.subtree_keys(child, 0)?;
            }
        }
        Ok(Some(before + 1))
    }

    /// The number of keys in the order. Reads every page.
    pub fn key_count(&self, file: &mut NtxFile) -> Result<u32, Error> {
        let root = file.header.root;
        file.subtree_keys(root, 0)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::Value as Json;

    use super::*;

    /// Every `.ntx` fixture with the dump of its order.
    fn orders() -> Vec<(PathBuf, Json)> {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/ntx");
        let mut orders = Vec::new();
        for dir in std::fs::read_dir(fixtures).unwrap() {
            let dir = dir.unwrap().path();
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_none_or(|e| e != "json") {
                    continue;
                }
                let dump: Json = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                for index in dump["indexes"].as_array().unwrap() {
                    let file = dir.join(index["file"].as_str().unwrap());
                    if file.extension().is_some_and(|e| e == "ntx") {
                        orders.push((file, index.clone()));
                    }
                }
            }
        }
        orders.sort_by(|a, b| a.0.cmp(&b.0));
        orders
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Walking each order forward and backward gives the keys the engine
    /// listed; key numbers count them, and every key seeks to itself.
    #[test]
    fn orders_match_the_engine() {
        let orders = orders();
        assert!(orders.len() >= 15, "found only {} NTX orders", orders.len());
        let mut keys_checked = 0;
        for (path, dump) in &orders {
            let name = path.display();
            let mut file = NtxFile::open(path).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            // The German static collations of `charset_german`, from the
            // original's files if this machine has them.
            if path.to_string_lossy().contains("charset_german") {
                let Some(dir) = crate::collation::tests::collation_dir() else {
                    continue;
                };
                let oem = path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("co");
                let (chr, entry, language) = match oem {
                    true => ("extend.chr", 14, "GERMAN"),
                    false => ("ansi.chr", 16, "German"),
                };
                let bytes = std::fs::read(dir.join(chr)).unwrap();
                let collation = crate::collation::Static::from_chr(&bytes, entry, language);
                file.set_weights(Some(collation.unwrap().weights));
            }
            assert_eq!(
                file.header.expression,
                dump["expression"].as_str().unwrap(),
                "{name}"
            );
            assert_eq!(
                file.header.condition,
                dump["condition"].as_str().unwrap(),
                "{name}"
            );
            assert_eq!(
                u64::from(file.header.key_length),
                dump["key_length"],
                "{name}"
            );
            assert_eq!(file.header.unique, dump["unique"] == true, "{name}");
            assert_eq!(file.header.descending, dump["descending"] == true, "{name}");
            let expected: Vec<(u64, String)> = dump["keys"]
                .as_array()
                .unwrap()
                .iter()
                .map(|k| {
                    (
                        k["recno"].as_u64().unwrap(),
                        k["key"].as_str().unwrap().to_lowercase(),
                    )
                })
                .collect();
            let mut cursor = Cursor::new();
            let mut walked = Vec::new();
            let mut key = cursor.first(&mut file).unwrap().cloned();
            while let Some(k) = key {
                walked.push((u64::from(k.record), hex(&k.bytes)));
                assert_eq!(
                    cursor.key_number(&mut file).unwrap(),
                    Some(walked.len() as u32),
                    "{name}"
                );
                key = cursor.next(&mut file).unwrap().cloned();
            }
            assert!(cursor.after_last(), "{name}");
            assert_eq!(walked, expected, "{name}");
            assert_eq!(
                cursor.key_count(&mut file).unwrap() as usize,
                expected.len(),
                "{name}"
            );
            let mut back = Vec::new();
            let mut key = cursor.prev(&mut file).unwrap().cloned();
            while let Some(k) = key {
                back.push((u64::from(k.record), hex(&k.bytes)));
                key = cursor.prev(&mut file).unwrap().cloned();
            }
            assert!(cursor.before_first(), "{name}");
            back.reverse();
            assert_eq!(back, expected, "{name}");
            for (record, key) in &expected {
                let bytes: Vec<u8> = (0..key.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&key[i..i + 2], 16).unwrap())
                    .collect();
                let found = cursor
                    .seek(&mut file, &bytes, Some(*record as u32))
                    .unwrap()
                    .cloned();
                assert_eq!(found.map(|k| u64::from(k.record)), Some(*record), "{name}");
                keys_checked += 1;
            }
        }
        assert!(keys_checked > 3000, "checked only {keys_checked} keys");
    }

    #[test]
    fn keys_of_values() {
        assert_eq!(number_key(1.5, 8, 2), b"00001.50");
        assert_eq!(number_key(-1.5, 8, 2), b",,,,+.',");
        assert_eq!(number_key(-999.99, 8, 2), b",,###.##");
        assert_eq!(number_key(-2147483647.0, 11, 0), b",*+(%($)&(%");
        assert_eq!(number_key(-0.001, 26, 15), b",,,,,,,,,,.,,+,,,,,,,,,,,,");
        assert_eq!(number_key(1.5, 11, 0), b"00000000002");
        assert_eq!(number_key(2.5, 11, 0), b"00000000003");
        assert_eq!(
            number_key(123456789.25, 26, 15),
            b"0123456789.250000000000000"
        );
        assert_eq!(number_key(0.0, 8, 2), b"00000.00");
        assert_eq!(number_key(123456.0, 4, 0), b"****");
        assert_eq!(date_key(2458066), b"20171108");
        assert_eq!(date_key(0), b"        ");
        assert_eq!(date_key(5373485), b"****0101");
    }

    #[test]
    fn rejects_other_files() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let dbf = fixtures.join("ntx/ntx_index/ntx_index.dbf");
        assert!(matches!(NtxFile::open(&dbf), Err(Error::Corrupt(_))));
    }
}
