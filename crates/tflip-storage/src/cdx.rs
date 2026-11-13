//! FoxPro indexes: compound `.cdx` files of several orders, and `.idx`
//! files of one, in the compact format of FoxPro 2 and Visual FoxPro.
//!
//! A file is made of 512-byte nodes. A tag header (1,024 bytes) holds the
//! root node, the key length, options, whether the order is descending, and
//! the key expression and `FOR` condition. A `.cdx` file starts with the
//! header of its tag directory, an order whose keys are the tags' names and
//! whose record numbers are the offsets of their headers; an `.idx` file
//! starts with its one tag's header.
//!
//! The tree is a B+ tree. An interior node holds, per child, the child's
//! last key, its record number and the child's offset (both big-endian). A
//! leaf holds its keys compressed: from its start, per key a bit field of
//! the record number, the count of leading bytes it shares with the key
//! before it, and the count of trailing pad bytes (spaces, or zeros in a
//! non-character key) it leaves out; the rest of each key is stored from
//! the node's end backwards. Leaves are linked to their siblings. Keys are
//! stored ascending, and equal keys in the order of their records; a
//! descending order walks them backwards.

use crate::bytes::{text, u16_at, u32_at};
use std::cmp::Ordering;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::order::{Error, Key, Tag};

/// The size of a node.
pub const NODE_SIZE: usize = 512;

/// The size of a tag header.
const HEADER_SIZE: usize = 1024;

/// No sibling.
const NONE: u32 = u32::MAX;

/// Tag header options (offset 0x0E).
mod options {
    pub const UNIQUE: u8 = 0x01;
    pub const NULLABLE: u8 = 0x02;
    pub const CANDIDATE: u8 = 0x04;
    pub const CONDITION: u8 = 0x08;
    pub const COMPOUND: u8 = 0x40;
    /// A full-text order, whose header names its options block
    /// (`fixtures/write/fts_cdx`).
    pub const FULL_TEXT: u8 = 0x10;
}

/// The tag header signature of an Advantage binary (bitmap) order.
const BINARY_SIGNATURE: u8 = 5;

/// A decoded node: its keys in stored order, and for an interior node each
/// key's child.
#[derive(Debug, Clone)]
struct Node {
    keys: Vec<Key>,
    /// Empty in a leaf.
    children: Vec<u32>,
    left: u32,
    right: u32,
}

impl Node {
    fn leaf(&self) -> bool {
        self.children.is_empty()
    }
}

/// An open `.cdx` or `.idx` file.
#[derive(Debug)]
pub struct CdxFile {
    pub(crate) file: File,
    /// The weights text keys are ordered by (a static collation's), if they
    /// are not the bytes themselves.
    pub(crate) weights: Option<Box<[u8; 256]>>,
    /// Whether the file is compound (`.cdx`).
    pub compound: bool,
    /// The orders, in the order they were created (by tag header offset).
    /// A tag's `root` is its root node; its `pad` is a space (zero with a
    /// collation) until the opener says the keys are not text; its
    /// `collation` the short name of a VFP collation (`GERMAN`).
    pub tags: Vec<Tag>,
}

impl CdxFile {
    /// Opens the `.cdx` or `.idx` file at `path` and reads its tags.
    pub fn open(path: &Path) -> Result<CdxFile, Error> {
        CdxFile::from_file(File::open(path).map_err(Error::Io)?, path)
    }

    /// Opens the `.cdx` or `.idx` file at `path` to read and write (see
    /// [`crate::maintain`]).
    pub fn open_for_writing(path: &Path) -> Result<CdxFile, Error> {
        let file = File::options()
            .read(true)
            .write(true)
            .open(path)
            .map_err(Error::Io)?;
        CdxFile::from_file(file, path)
    }

    fn from_file(file: File, path: &Path) -> Result<CdxFile, Error> {
        let mut cdx = CdxFile {
            file,
            weights: None,
            compound: false,
            tags: Vec::new(),
        };
        let header = cdx.read(0, HEADER_SIZE)?;
        if u16_at(&header, 0x0C) == 0 || !(u32_at(&header, 0) as usize).is_multiple_of(NODE_SIZE) {
            return Err(Error::Corrupt("not a CDX or IDX file"));
        }
        cdx.compound = header[0x0E] & options::COMPOUND != 0;
        if !cdx.compound {
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned());
            cdx.tags = vec![cdx.tag(0, name.unwrap_or_default())?];
            return Ok(cdx);
        }
        // The tag directory: names and header offsets.
        let directory = Tag {
            name: String::new(),
            header_page: 0,
            root: u32_at(&header, 0),
            key_length: u16_at(&header, 0x0C),
            options: header[0x0E],
            pad: b' ',
            flags: 0,
            descending: false,
            expression: String::new(),
            condition: String::new(),
            collation: String::new(),
            locale: String::new(),
            uchar_width: 0,
            decimals: 0,
            fts_block: 0,
            keeps_score: false,
        };
        let mut cursor = Cursor::new(&directory);
        let mut entries = Vec::new();
        let mut key = cursor.first(&mut cdx)?.cloned();
        while let Some(found) = key {
            let name = text(&found.bytes).trim_end().to_owned();
            entries.push((found.record, name));
            if entries.len() > 10_000 {
                return Err(Error::Corrupt("CDX tag directory loops"));
            }
            key = cursor.next(&mut cdx)?.cloned();
        }
        entries.sort();
        for (offset, name) in entries {
            let tag = cdx.tag(offset, name)?;
            cdx.tags.push(tag);
        }
        Ok(cdx)
    }

    /// Orders keys by `weights` (a static collation's), byte by byte: those
    /// of text (a tag padded with spaces). Keys of a dynamic collation are
    /// weights already, and keys of numbers and dates their bytes, ordered as
    /// bytes (`fixtures/write/aofs_static_keys`).
    pub fn set_weights(&mut self, weights: Option<[u8; 256]>) {
        self.weights = weights
            .filter(|w| w.iter().enumerate().any(|(i, &b)| usize::from(b) != i))
            .map(Box::new);
    }

    /// The tag whose header is at `offset`, named `name`.
    fn tag(&mut self, offset: u32, name: String) -> Result<Tag, Error> {
        let header = self.read(offset, HEADER_SIZE)?;
        let options = header[0x0E];
        let key_length = u16_at(&header, 0x0C);
        if key_length == 0 || usize::from(key_length) > 240 {
            return Err(Error::Corrupt("CDX key length out of range"));
        }
        let expression_length = usize::from(u16_at(&header, 0x1FE));
        let pool = &header[0x200..];
        let expression = text(&pool[..expression_length.min(pool.len())]);
        let condition = match options & options::CONDITION {
            0 => String::new(),
            _ => text(pool.get(expression_length..).unwrap_or_default()),
        };
        // A VFP collation's short name (`GERMAN`), whose keys are weights
        // padded with zeros; none for the machine order.
        let collation = text(&header[0x1EE..0x1F6]).trim().to_owned();
        let collation = match collation
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b == b'_')
        {
            true => collation,
            false => String::new(),
        };
        let full_text = options & options::FULL_TEXT != 0 && header[0x11] == 1;
        let (fts_block, keeps_score) = match full_text {
            true => {
                let block = u32_at(&header, 0x14);
                let bytes = self.read(block, NODE_SIZE)?;
                let options = u32_at(&bytes, 8);
                (block, options & crate::adi::FTS_KEEP_SCORE != 0)
            }
            false => (0, false),
        };
        Ok(Tag {
            name,
            header_page: offset,
            root: u32_at(&header, 0),
            key_length,
            options: if options & options::UNIQUE != 0 {
                crate::order::options::UNIQUE
            } else {
                0
            } | if condition.is_empty() {
                0
            } else {
                crate::order::options::CONDITION
            },
            pad: if collation.is_empty() { b' ' } else { 0 },
            flags: match header[0x0F] {
                BINARY_SIGNATURE => crate::order::flags::BINARY,
                _ => 0,
            } | if options & options::CANDIDATE != 0 {
                crate::order::flags::CANDIDATE
            } else {
                0
            } | if options & options::NULLABLE != 0 {
                crate::order::flags::NULLABLE
            } else {
                0
            } | if full_text {
                crate::order::flags::FULL_TEXT
            } else {
                0
            },
            descending: u16_at(&header, 0x1F6) != 0,
            expression,
            condition,
            collation,
            locale: String::new(),
            uchar_width: 0,
            decimals: 0,
            fts_block,
            keeps_score,
        })
    }

    /// The options block of a full-text order `tag` ([`crate::adi::IndexFile::fts_block`]),
    /// which the tag header names by its offset in the file.
    pub fn fts_block(&mut self, tag: &Tag) -> Result<Option<crate::create::FtsBlock>, Error> {
        if !tag.full_text() {
            return Ok(None);
        }
        let mut bytes = self.read(tag.fts_block, NODE_SIZE)?;
        let length = u32_at(&bytes, 4) as usize;
        while bytes.len() < length {
            let next = tag.fts_block + bytes.len() as u32;
            bytes.extend(self.read(next, NODE_SIZE)?);
        }
        crate::adi::parse_fts_block(&bytes).map(Some)
    }

    fn read(&mut self, offset: u32, length: usize) -> Result<Vec<u8>, Error> {
        let mut bytes = vec![0; length];
        self.file.seek(SeekFrom::Start(u64::from(offset)))?;
        self.file
            .read_exact(&mut bytes)
            .map_err(|_| Error::Corrupt("CDX file ends inside a node"))?;
        Ok(bytes)
    }

    /// Reads and decodes the node at `offset` of an order whose keys are
    /// `key_length` bytes, padded with `pad`; of a `binary` order, whose
    /// leaves are bitmaps.
    fn node(
        &mut self,
        offset: u32,
        key_length: usize,
        pad: u8,
        binary: bool,
    ) -> Result<Node, Error> {
        if !(offset as usize).is_multiple_of(NODE_SIZE) {
            return Err(Error::Corrupt("CDX node offset not on a node"));
        }
        let bytes = self.read(offset, NODE_SIZE)?;
        let attributes = u16_at(&bytes, 0);
        let count = usize::from(u16_at(&bytes, 2));
        let left = u32_at(&bytes, 4);
        let right = u32_at(&bytes, 8);
        let mut keys = Vec::with_capacity(count);
        if attributes & 0x02 == 0 {
            // Interior: key, record (BE), child (BE); a binary order's
            // entries have no key, their record the last its child covers.
            let key_length = if binary { 0 } else { key_length };
            let entry = key_length + 8;
            if 12 + count * entry > NODE_SIZE {
                return Err(Error::Corrupt("CDX interior node holds too many keys"));
            }
            let mut children = Vec::with_capacity(count);
            for i in 0..count {
                let at = 12 + i * entry;
                let record = u32::from_be_bytes(
                    bytes[at + key_length..at + key_length + 4]
                        .try_into()
                        .unwrap(),
                );
                let child =
                    u32::from_be_bytes(bytes[at + key_length + 4..at + entry].try_into().unwrap());
                keys.push(Key {
                    bytes: bytes[at..at + key_length].to_vec(),
                    record,
                });
                children.push(child);
            }
            return Ok(Node {
                keys,
                children,
                left,
                right,
            });
        }
        if binary {
            // A bitmap of `count` records from the one at 12 (big-endian),
            // a bit set for `T`; record 0 is no record.
            let first = u32::from_be_bytes(bytes[12..16].try_into().unwrap());
            if count > (NODE_SIZE - 16) * 8 {
                return Err(Error::Corrupt("CDX bitmap overflows its node"));
            }
            for i in 0..count {
                let record = first + i as u32;
                let set = bytes[16 + i / 8] & (1 << (i % 8)) != 0;
                if record != 0 {
                    keys.push(Key {
                        bytes: vec![if set { b'T' } else { b'F' }],
                        record,
                    });
                }
            }
            return Ok(Node {
                keys,
                children: Vec::new(),
                left,
                right,
            });
        }
        let record_mask = u32_at(&bytes, 14);
        let (duplicate_mask, trailing_mask) = (u32::from(bytes[18]), u32::from(bytes[19]));
        let (record_bits, duplicate_bits) = (u32::from(bytes[20]), u32::from(bytes[21]));
        let entry = usize::from(bytes[23]);
        if entry == 0
            || entry > 8
            || 24 + count * entry > NODE_SIZE
            || record_bits + duplicate_bits >= 64
        {
            return Err(Error::Corrupt("CDX leaf node layout out of range"));
        }
        let mut end = NODE_SIZE;
        let mut previous: Vec<u8> = Vec::new();
        for i in 0..count {
            let at = 24 + i * entry;
            let mut field = [0u8; 8];
            field[..entry].copy_from_slice(&bytes[at..at + entry]);
            let field = u64::from_le_bytes(field);
            let record = (field as u32) & record_mask;
            let duplicate = ((field >> record_bits) as u32 & duplicate_mask) as usize;
            let trailing =
                ((field >> (record_bits + duplicate_bits)) as u32 & trailing_mask) as usize;
            let stored = key_length
                .checked_sub(duplicate + trailing)
                .ok_or(Error::Corrupt("CDX leaf key longer than its order's keys"))?;
            if duplicate > previous.len() || stored > end - (24 + count * entry) {
                return Err(Error::Corrupt("CDX leaf key data out of range"));
            }
            end -= stored;
            let mut key = previous[..duplicate].to_vec();
            key.extend_from_slice(&bytes[end..end + stored]);
            key.resize(key_length, pad);
            previous = key.clone();
            keys.push(Key { bytes: key, record });
        }
        Ok(Node {
            keys,
            children: Vec::new(),
            left,
            right,
        })
    }
}

/// How the entry `key` compares with `sought` and `record`, in stored
/// order: by the `weights` of its first `sought.len()` bytes (the bytes
/// themselves without), then, for a full key, by record.
fn compare(key: &Key, sought: &[u8], record: Option<u32>, weights: Option<&[u8; 256]>) -> Ordering {
    let length = sought.len().min(key.bytes.len());
    let (key_bytes, sought_bytes) = (&key.bytes[..length], &sought[..length]);
    let by_key = crate::collation::weighed_cmp(key_bytes, sought_bytes, weights);
    by_key.then_with(|| match record {
        Some(record) if sought.len() >= key.bytes.len() => key.record.cmp(&record),
        _ => Ordering::Equal,
    })
}

/// Where a cursor is, in stored order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Start,
    On { node: u32, slot: usize },
    End,
}

/// A position in an order of a CDX or IDX file, moving through its keys in
/// index order (ascending, or reversed for a descending order), as
/// [`crate::adi::Cursor`] does. It holds the leaf it is on, decoded.
#[derive(Debug, Clone)]
pub struct Cursor {
    root: u32,
    key_length: usize,
    pad: u8,
    /// A binary order, whose leaves are bitmaps.
    binary: bool,
    descending: bool,
    place: Place,
    leaf: Option<Node>,
}

/// The deepest tree walked.
const MAX_DEPTH: usize = 32;

impl Cursor {
    /// A cursor before the first key of `tag`.
    pub fn new(tag: &Tag) -> Cursor {
        Cursor {
            root: tag.root,
            key_length: usize::from(tag.key_length),
            pad: tag.pad,
            binary: tag.binary(),
            descending: tag.descending,
            place: if tag.descending {
                Place::End
            } else {
                Place::Start
            },
            leaf: None,
        }
    }

    pub fn key(&self) -> Option<&Key> {
        match self.place {
            Place::On { slot, .. } => self.leaf.as_ref()?.keys.get(slot),
            _ => None,
        }
    }

    pub fn before_first(&self) -> bool {
        self.place
            == if self.descending {
                Place::End
            } else {
                Place::Start
            }
    }

    pub fn after_last(&self) -> bool {
        self.place
            == if self.descending {
                Place::Start
            } else {
                Place::End
            }
    }

    fn node(&self, file: &mut CdxFile, offset: u32) -> Result<Node, Error> {
        file.node(offset, self.key_length, self.pad, self.binary)
    }

    /// Lands on `slot` of `leaf` (at `offset`); off the keys if the slot is
    /// past its keys, which only an empty root leaf has.
    fn land(&mut self, offset: u32, leaf: Node, slot: usize, past_end: bool) {
        if slot < leaf.keys.len() {
            self.place = Place::On { node: offset, slot };
            self.leaf = Some(leaf);
        } else {
            self.place = if past_end { Place::End } else { Place::Start };
            self.leaf = None;
        }
    }

    /// Descends from the root along the first (or `last`) children.
    fn stored_end(&mut self, file: &mut CdxFile, last: bool) -> Result<(), Error> {
        let mut offset = self.root;
        for _ in 0..MAX_DEPTH {
            let node = self.node(file, offset)?;
            if node.leaf() {
                let slot = if last {
                    node.keys.len().saturating_sub(1)
                } else {
                    0
                };
                let empty = node.keys.is_empty();
                self.land(offset, node, if empty { 0 } else { slot }, !last);
                return Ok(());
            }
            offset = match if last {
                node.children.last()
            } else {
                node.children.first()
            } {
                Some(&child) => child,
                None => {
                    self.place = if last { Place::Start } else { Place::End };
                    self.leaf = None;
                    return Ok(());
                }
            };
        }
        Err(Error::Corrupt("CDX tree too deep"))
    }

    /// Moves one key on in stored order (`forward`) or back.
    fn stored_step(&mut self, file: &mut CdxFile, forward: bool) -> Result<(), Error> {
        let (node, slot) = match self.place {
            Place::Start if forward => return self.stored_end(file, false),
            Place::End if !forward => return self.stored_end(file, true),
            Place::Start | Place::End => return Ok(()),
            Place::On { node, slot } => (node, slot),
        };
        let leaf = self.leaf.as_ref().unwrap();
        if forward && slot + 1 < leaf.keys.len() {
            self.place = Place::On {
                node,
                slot: slot + 1,
            };
            return Ok(());
        }
        if !forward && slot > 0 {
            self.place = Place::On {
                node,
                slot: slot - 1,
            };
            return Ok(());
        }
        let mut sibling = if forward { leaf.right } else { leaf.left };
        // Skip empty leaves.
        for _ in 0..1_000_000 {
            if sibling == NONE {
                self.place = if forward { Place::End } else { Place::Start };
                self.leaf = None;
                return Ok(());
            }
            let next = self.node(file, sibling)?;
            if !next.keys.is_empty() {
                let slot = if forward { 0 } else { next.keys.len() - 1 };
                self.land(sibling, next, slot, forward);
                return Ok(());
            }
            sibling = if forward { next.right } else { next.left };
        }
        Err(Error::Corrupt("CDX leaf siblings loop"))
    }

    /// Moves to the first stored entry for which `before` does not hold, or
    /// past the end; `before` holds for a first part of the entries. An
    /// interior entry is its child's last key, so the entry is in the first
    /// child whose last key `before` does not hold for.
    fn stored_bound(
        &mut self,
        file: &mut CdxFile,
        before: impl Fn(&Key) -> bool,
    ) -> Result<(), Error> {
        let mut offset = self.root;
        for _ in 0..MAX_DEPTH {
            let node = self.node(file, offset)?;
            let slot = node.keys.partition_point(&before);
            if node.leaf() {
                if slot < node.keys.len() {
                    self.land(offset, node, slot, true);
                } else {
                    // On to the next leaf's first key.
                    let last = node.keys.len();
                    self.place = Place::On {
                        node: offset,
                        slot: last.saturating_sub(1),
                    };
                    let empty = node.keys.is_empty();
                    self.leaf = Some(node);
                    if empty {
                        self.place = Place::End;
                        self.leaf = None;
                    } else {
                        self.stored_step(file, true)?;
                    }
                }
                return Ok(());
            }
            match node.children.get(slot) {
                Some(&child) => offset = child,
                None => {
                    self.place = Place::End;
                    self.leaf = None;
                    return Ok(());
                }
            }
        }
        Err(Error::Corrupt("CDX tree too deep"))
    }

    pub fn first(&mut self, file: &mut CdxFile) -> Result<Option<&Key>, Error> {
        self.stored_end(file, self.descending)?;
        Ok(self.key())
    }

    pub fn last(&mut self, file: &mut CdxFile) -> Result<Option<&Key>, Error> {
        self.stored_end(file, !self.descending)?;
        Ok(self.key())
    }

    pub fn next(&mut self, file: &mut CdxFile) -> Result<Option<&Key>, Error> {
        self.stored_step(file, !self.descending)?;
        Ok(self.key())
    }

    pub fn prev(&mut self, file: &mut CdxFile) -> Result<Option<&Key>, Error> {
        self.stored_step(file, self.descending)?;
        Ok(self.key())
    }

    /// Moves to the first key, in index order, that is not before `key` and
    /// `record` (see [`crate::adi::Cursor::seek`]).
    pub fn seek(
        &mut self,
        file: &mut CdxFile,
        key: &[u8],
        record: Option<u32>,
    ) -> Result<Option<&Key>, Error> {
        let weights = file.weights.clone().filter(|_| self.pad == b' ');
        let weights = weights.as_deref();
        if self.descending {
            // The last stored entry at or below: the one before the first
            // above.
            self.stored_bound(file, |entry| {
                compare(entry, key, record, weights) != Ordering::Greater
            })?;
            self.stored_step(file, false)?;
        } else {
            self.stored_bound(file, |entry| {
                compare(entry, key, record, weights) == Ordering::Less
            })?;
        }
        Ok(self.key())
    }

    /// The place of the current key in index order, 1-based; `None` off the
    /// keys. Counts the keys of the leaves before the current one.
    pub fn key_number(&self, file: &mut CdxFile) -> Result<Option<u32>, Error> {
        let Place::On { slot, .. } = self.place else {
            return Ok(None);
        };
        // A binary order's bits start at record 0, which counts.
        if self.binary {
            return Ok(self.key().map(|key| key.record + 1));
        }
        let leaf = self.leaf.as_ref().unwrap();
        let (mut count, mut sibling) = match self.descending {
            false => (slot, leaf.left),
            true => (leaf.keys.len() - 1 - slot, leaf.right),
        };
        for _ in 0..1_000_000 {
            if sibling == NONE {
                return Ok(Some(count as u32 + 1));
            }
            let node = self.node(file, sibling)?;
            count += node.keys.len();
            sibling = if self.descending {
                node.right
            } else {
                node.left
            };
        }
        Err(Error::Corrupt("CDX leaf siblings loop"))
    }

    /// The number of keys in the order. Reads every leaf.
    pub fn key_count(&self, file: &mut CdxFile) -> Result<u32, Error> {
        let mut first = self.clone();
        first.descending = false;
        first.stored_end(file, false)?;
        let Place::On { .. } = first.place else {
            return Ok(0);
        };
        // A binary order's bits start at record 0, which counts.
        let mut count = first.leaf.as_ref().unwrap().keys.len() + usize::from(self.binary);
        let mut sibling = first.leaf.as_ref().unwrap().right;
        for _ in 0..1_000_000 {
            if sibling == NONE {
                return Ok(count as u32);
            }
            let node = self.node(file, sibling)?;
            count += node.keys.len();
            sibling = node.right;
        }
        Err(Error::Corrupt("CDX leaf siblings loop"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::Value as Json;

    use super::*;

    /// Every CDX and IDX fixture order with its dump, and whether its
    /// fixture has a collation of its own.
    fn orders() -> Vec<(PathBuf, Json)> {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let mut orders = Vec::new();
        for kind in ["cdx", "vfp"] {
            for dir in std::fs::read_dir(fixtures.join(kind)).unwrap() {
                let dir = dir.unwrap().path();
                for entry in std::fs::read_dir(&dir).unwrap() {
                    let path = entry.unwrap().path();
                    if path.extension().is_none_or(|e| e != "json") {
                        continue;
                    }
                    let dump: Json =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    for index in dump["indexes"].as_array().unwrap() {
                        let file = dir.join(index["file"].as_str().unwrap());
                        orders.push((file, index.clone()));
                    }
                }
            }
        }
        orders.sort_by(|a, b| (&a.0, a.1["name"].as_str()).cmp(&(&b.0, b.1["name"].as_str())));
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
        assert!(orders.len() >= 40, "found only {} orders", orders.len());
        let mut keys_checked = 0;
        let mut orders_checked = 0;
        for (path, dump) in &orders {
            let name = format!("{} {}", path.display(), dump["name"]);
            let mut file = CdxFile::open(path).unwrap_or_else(|e| panic!("{name}: {e:?}"));
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
            let tag = file
                .tags
                .iter()
                .find(|t| t.name.eq_ignore_ascii_case(dump["name"].as_str().unwrap()))
                .unwrap_or_else(|| panic!("{name}: no such tag"))
                .clone();
            assert_eq!(
                tag.expression,
                dump["expression"].as_str().unwrap(),
                "{name}"
            );
            assert_eq!(tag.condition, dump["condition"].as_str().unwrap(), "{name}");
            assert_eq!(u64::from(tag.key_length), dump["key_length"], "{name}");
            assert_eq!(tag.unique(), dump["unique"] == true, "{name}");
            let candidate = tag.flags & crate::order::flags::CANDIDATE != 0;
            assert_eq!(candidate, dump["candidate"] == true, "{name}");
            assert_eq!(tag.descending, dump["descending"] == true, "{name}");
            if dump["fts"] == true {
                continue;
            }
            let mut tag = tag;
            if dump["key_type"] != 4 {
                tag.pad = 0;
            }
            if !tag.collation.is_empty() {
                let collation = dump["collation"].as_str().unwrap();
                assert!(
                    collation.starts_with(&format!("{}_", tag.collation)),
                    "{name}"
                );
            }
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
            let mut cursor = Cursor::new(&tag);
            let mut walked = Vec::new();
            let mut key = cursor.first(&mut file).unwrap().cloned();
            while let Some(k) = key {
                walked.push((u64::from(k.record), hex(&k.bytes)));
                // Key numbers read every leaf before the key: some of them.
                // (A binary order's count from record 0.)
                if walked.len() < 100 || walked.len() % 97 == 0 {
                    let number = cursor.key_number(&mut file).unwrap();
                    let expected = if tag.binary() {
                        k.record + 1
                    } else {
                        walked.len() as u32
                    };
                    assert_eq!(number, Some(expected), "{name}");
                }
                key = cursor.next(&mut file).unwrap().cloned();
            }
            assert!(cursor.after_last(), "{name}");
            assert_eq!(walked, expected, "{name}");
            let count = expected.len() + usize::from(tag.binary());
            assert_eq!(
                cursor.key_count(&mut file).unwrap() as usize,
                count,
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
            // A binary order's keys are in record order: they seek nothing.
            for (record, key) in expected.iter().filter(|_| !tag.binary()) {
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
            orders_checked += 1;
        }
        assert!(orders_checked >= 30, "checked only {orders_checked} orders");
        assert!(keys_checked > 5000, "checked only {keys_checked} keys");
    }
}
