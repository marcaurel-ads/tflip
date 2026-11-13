//! ADI indexes (`.adi`): compound indexes of one or more orders (tags), each
//! a B+ tree of binary sort keys.
//!
//! An ADI file is a sequence of pages of one size (512 to 8,192 bytes);
//! pointers are page numbers. It starts with a 1,024-byte file header,
//! followed by the tag directory (a leaf mapping tag names to tag header
//! pages) and, per order, a 1,024-byte tag header and the tree's pages.
//! Header integers are little-endian; record numbers and child pointers
//! inside nodes are big-endian.
//!
//! Keys are ordered by their bytes, equal keys by record number. Each entry
//! of an interior node holds the last key (and record) of its child subtree;
//! leaves link to their left and right siblings. [`IndexFile::keys`] reads
//! all keys of an order; a [`Cursor`] moves through them and seeks a key
//! reading only the pages on its way.

use crate::bytes::{text, trim_nul, u16_at, u32_at, u32_be_at};
use std::cmp::Ordering;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::cp1252;
use crate::order::{Error, Key, Tag, flags, options};
use crate::table::RecordCipher;

/// The size of the file header and of each tag header with its expression
/// pool.
const HEADER_SIZE: usize = 1024;

/// The size of a node header: attributes, key count, and the left and right
/// sibling pages.
const NODE_HEADER_SIZE: usize = 12;

/// The size of a compact leaf's header, up to its first entry.
const LEAF_HEADER_SIZE: usize = 24;

/// No page.
const NONE: u32 = 0xFFFF_FFFF;

/// The `ADS_FTS_INDEX` option bit, implied for every full-text order and so not
/// stored in its options word.
const FTS_INDEX_BIT: u32 = 0x20;

/// `ADS_FTS_KEEP_SCORE`: the order's leaf entries count each word's
/// occurrences in its record.
pub const FTS_KEEP_SCORE: u32 = 0x100;

/// The parsing options of a full-text order (see [`IndexFile::fts_options`] and
/// `AdsGetFTSIndexInfo`): what the engine tokenizes and matches `CONTAINS` with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtsOptions {
    /// The indexed field, as the order's expression (`F<n>`).
    pub field: String,
    /// The shortest and longest word the index keeps.
    pub min_word: u32,
    pub max_word: u32,
    /// The option flags (`ADS_FTS_*`), the implied `ADS_FTS_INDEX` bit included.
    pub options: u32,
    /// The characters that split words.
    pub delimiters: String,
    /// The noise (stop) words, space-separated, as stored.
    pub noise: String,
    /// The characters dropped from words.
    pub drop: String,
    /// The conditional characters (kept only between word characters).
    pub conditional: String,
}

/// The parts of a file or tag header that are read.
struct Header {
    root: u32,
    key_length: u16,
    options: u8,
    pad: u8,
    page_size: u16,
    flags: u32,
}

impl Header {
    fn parse(bytes: &[u8]) -> Header {
        let options = bytes[0x0E];
        Header {
            root: u32_at(bytes, 0x00),
            key_length: u16_at(bytes, 0x0C),
            options,
            pad: bytes[0x0F],
            page_size: u16_at(bytes, 0x12),
            flags: if options & options::FLAGS != 0 {
                u32_at(bytes, 0x18)
            } else {
                0
            },
        }
    }
}

/// An open index file.
#[derive(Debug)]
pub struct IndexFile {
    pub(crate) file: File,
    pub(crate) page_size: u32,
    /// The CRC-32 of the collation's byte-to-weight table the file's keys were
    /// built under (file header 0x14; see [`crate::create::collation_crc`]).
    /// The engine refuses to open an index whose keys a different collation
    /// weighed (`AE_INDEX_COLLATION_MISMATCH`).
    pub collation_crc: u32,
    /// The orders, in the order they were created (by tag header page).
    pub tags: Vec<Tag>,
    /// How an encrypted index's tree pages are deciphered on read; `None`
    /// leaves them as stored.
    pub(crate) cipher: RecordCipher,
    /// An AES index's write IV counter, shared with the table's records so an
    /// index page never reuses an IV. Set by [`IndexFile::set_aes_counter`]
    /// before maintenance; its own fresh counter otherwise.
    pub(crate) aes_iv: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// How many record writes changed each order's keys while the file was
    /// open, by tag header page: a reindex writes the count into the tag
    /// header (`fixtures/write/cdx_custom_reindex`).
    pub(crate) changes: std::collections::HashMap<u32, u32>,
}

impl IndexFile {
    /// Opens the index at `path` and reads its orders.
    pub fn open(path: &Path) -> Result<IndexFile, Error> {
        IndexFile::open_with(path, false, None)
    }

    /// Opens an encrypted index at `path`, deciphering its tree pages the way
    /// the table is (`cipher`, the same its records use).
    pub fn open_encrypted(path: &Path, cipher: &RecordCipher) -> Result<IndexFile, Error> {
        IndexFile::open_with(path, false, Some(cipher))
    }

    /// Opens the index at `path` to read and write (see
    /// [`crate::maintain`]).
    pub fn open_for_writing(path: &Path) -> Result<IndexFile, Error> {
        IndexFile::open_with(path, true, None)
    }

    /// Opens an encrypted index at `path` to read and write, deciphering and
    /// enciphering its tree pages with the table's `cipher` (see
    /// [`crate::maintain`]); share the table's IV counter with
    /// [`IndexFile::set_aes_counter`].
    pub fn open_encrypted_for_writing(
        path: &Path,
        cipher: &RecordCipher,
    ) -> Result<IndexFile, Error> {
        IndexFile::open_with(path, true, Some(cipher))
    }

    /// Reads the file at `path`, written anew since it was opened (a reindex
    /// to other pages), again: its page size, collation and orders. The
    /// cipher and IV counter stay, which only the table knows, and so do the
    /// counts of the orders' changes, which go on counting
    /// (`fixtures/write/fts_rebuild`).
    pub fn reopened(&mut self, path: &Path) -> Result<(), Error> {
        let fresh = IndexFile::open_with(path, true, None)?;
        let changes = std::mem::take(&mut self.changes);
        for tag in &fresh.tags {
            let before = self.tags.iter().find(|t| t.name == tag.name);
            if let Some(count) = before.and_then(|t| changes.get(&t.header_page)) {
                self.changes.insert(tag.header_page, *count);
            }
        }
        self.file = fresh.file;
        self.page_size = fresh.page_size;
        self.collation_crc = fresh.collation_crc;
        self.tags = fresh.tags;
        Ok(())
    }

    /// Share the table's AES write IV counter, so pages this index writes in
    /// maintenance draw IVs that never collide with the table's records (both
    /// use the same key and nonce). No effect on an unencrypted index.
    pub fn set_aes_counter(&mut self, counter: std::sync::Arc<std::sync::atomic::AtomicU64>) {
        self.aes_iv = counter;
    }

    fn open_with(
        path: &Path,
        write: bool,
        cipher: Option<&RecordCipher>,
    ) -> Result<IndexFile, Error> {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(write)
            .open(path)
            .map_err(Error::Io)?;
        let mut header = [0; HEADER_SIZE];
        file.read_exact(&mut header)?;
        let file_header = Header::parse(&header);
        if u16_at(&header, 0x10) != 0x0400 {
            return Err(Error::Corrupt("not an ADI index"));
        }
        // `CREATE INDEX … PAGESIZE n` makes pages of any size in the range
        // (`fixtures/sql/ddopts`: 1000, 513).
        if !(512..=8192).contains(&file_header.page_size) {
            return Err(Error::Corrupt("invalid page size"));
        }
        // An encrypted index's tree pages are enciphered; the file header and
        // the orders' tag headers stay clear. An RC4 index checks the password
        // against "advantage" at header 0x20, item 0x2D; an AES index uses the
        // table's key (its header 0x20 holds the table's verifier).
        let cipher = if file_header.flags & flags::ENCRYPTED != 0 {
            let cipher = cipher.ok_or(Error::Encrypted)?;
            if let RecordCipher::Rc4(password) = cipher {
                let mut check: [u8; 20] = header[0x20..0x34].try_into().unwrap();
                crate::crypto::rc4_crypt(password, 0x2D, &mut check);
                if !check.starts_with(b"advantage") {
                    return Err(Error::Encrypted);
                }
            }
            cipher.clone()
        } else {
            RecordCipher::None
        };
        let mut index = IndexFile {
            file,
            page_size: u32::from(file_header.page_size),
            collation_crc: u32_at(&header, 0x14),
            tags: Vec::new(),
            cipher,
            aes_iv: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            changes: std::collections::HashMap::new(),
        };
        let directory = TreeRef {
            root: file_header.root,
            key_length: file_header.key_length,
            pad: file_header.pad,
            binary: false,
        };
        let mut tags = Vec::new();
        for entry in index.walk(&directory)? {
            let name = cp1252::decode(&entry.bytes)
                .trim_end_matches(' ')
                .to_owned();
            tags.push(index.read_tag(name, entry.record)?);
        }
        tags.sort_by_key(|tag| tag.header_page);
        index.tags = tags;
        Ok(index)
    }

    /// The page size in bytes.
    pub fn page_size(&self) -> u32 {
        self.page_size
    }

    /// The parsing options of a full-text order `tag` (`AdsGetFTSIndexInfo`):
    /// its word-length bounds, option flags, and the delimiter, noise, drop and
    /// conditional character sets the engine tokenizes with. Stored in a block
    /// of pages of its own, which the tag header names; `None` for an order
    /// that is not full-text. (Derived black-box, verified against `fixtures/sql/fts`.)
    pub fn fts_options(&mut self, tag: &Tag) -> Result<Option<FtsOptions>, Error> {
        let Some(block) = self.fts_block(tag)? else {
            return Ok(None);
        };
        Ok(Some(fts_options_of(tag, block)))
    }

    /// The options block of a full-text order `tag` as it is stored (see
    /// [`crate::create::FtsBlock`]); `None` for an order that is not
    /// full-text. It starts on a page of its own, which the tag header names,
    /// and may run on to the next ones.
    pub fn fts_block(&mut self, tag: &Tag) -> Result<Option<crate::create::FtsBlock>, Error> {
        if !tag.full_text() {
            return Ok(None);
        }
        // The options block stays in the clear in an encrypted index.
        let mut bytes = self.raw_page(tag.fts_block)?;
        let length = u32_at(&bytes, 4) as usize;
        let mut next = tag.fts_block + 1;
        while bytes.len() < length {
            bytes.extend(self.raw_page(next)?);
            next += 1;
        }
        parse_fts_block(&bytes).map(Some)
    }

    /// The keys of `tag` in index order: ascending, or reversed for a
    /// descending order (whose keys are stored ascending, so equal keys come
    /// by descending record number). A binary order has the key of its pad
    /// byte (`F`) for every record not marked deleted in its bitmaps.
    pub fn keys(&mut self, tag: &Tag) -> Result<Vec<Key>, Error> {
        let mut keys = self.walk(&TreeRef::of(tag))?;
        if tag.descending {
            keys.reverse();
        }
        Ok(keys)
    }

    /// Reads page `page` as it is stored.
    fn raw_page(&mut self, page: u32) -> Result<Vec<u8>, Error> {
        let mut bytes = vec![0; self.page_size as usize];
        self.file
            .seek(SeekFrom::Start(u64::from(page) * u64::from(self.page_size)))?;
        self.file.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    /// Reads page `page`.
    pub(crate) fn page(&mut self, page: u32) -> Result<Vec<u8>, Error> {
        let mut bytes = self.raw_page(page)?;
        // A tree page of an encrypted index is one cipher stream at its page
        // number (see [`crate::crypto`]); the header and tag headers, read
        // elsewhere, stay clear.
        match &self.cipher {
            RecordCipher::None => {}
            RecordCipher::Rc4(password) => crate::crypto::rc4_crypt(password, page, &mut bytes),
            RecordCipher::Aes { key, nonce } => aes_decipher_page(key, nonce, &mut bytes),
        }
        Ok(bytes)
    }

    /// Reads the tag header at `page` of the order `name`.
    fn read_tag(&mut self, name: String, page: u32) -> Result<Tag, Error> {
        let mut bytes = [0; HEADER_SIZE];
        self.file
            .seek(SeekFrom::Start(u64::from(page) * u64::from(self.page_size)))?;
        self.file.read_exact(&mut bytes)?;
        let header = Header::parse(&bytes);
        let condition_length = usize::from(u16_at(&bytes, 0x1FA));
        let expression_length = usize::from(u16_at(&bytes, 0x1FE));
        let pool = &bytes[0x200..];
        if expression_length == 0 || expression_length + condition_length > pool.len() {
            return Err(Error::Corrupt("expressions do not fit their pool"));
        }
        let expression = text(&pool[..expression_length]);
        let condition = text(&pool[expression_length..expression_length + condition_length]);
        let (fts_block, keeps_score) = match header.flags & crate::order::flags::FULL_TEXT {
            0 => (0, false),
            _ => {
                let block = u32_at(&bytes, 0x1C);
                let options = self
                    .page(block)?
                    .get(0x08..0x0C)
                    .map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
                (block, options & FTS_KEEP_SCORE != 0)
            }
        };
        Ok(Tag {
            name,
            header_page: page,
            root: header.root,
            key_length: header.key_length,
            options: header.options,
            pad: header.pad,
            flags: header.flags,
            descending: u16_at(&bytes, 0x1F6) == 1,
            expression,
            condition,
            collation: text(&bytes[0x34..0x1F6]),
            locale: text(&bytes[crate::order::locale::OFFSET..crate::order::locale::END]),
            uchar_width: bytes[crate::order::locale::WIDTH],
            decimals: 0,
            fts_block,
            keeps_score,
        })
    }

    /// The keys of a tree in stored (ascending) order: down the first
    /// children to the first leaf, then through the leaves' right siblings.
    fn walk(&mut self, tree: &TreeRef) -> Result<Vec<Key>, Error> {
        let pages = self.file.metadata()?.len() / u64::from(self.page_size);
        let mut keys = Vec::new();
        let mut page = tree.root;
        let mut visited = 0;
        loop {
            visited += 1;
            if page == NONE || u64::from(page) >= pages || visited > pages {
                return Err(Error::Corrupt("tree page out of the file"));
            }
            let node = self.page(page)?;
            if u16_at(&node, 0) & 0x02 == 0 {
                // Interior: every entry is a key (none in a binary order),
                // its record and a child.
                if u16_at(&node, 2) == 0 {
                    return Err(Error::Corrupt("empty interior node"));
                }
                let key_length = if tree.binary {
                    0
                } else {
                    usize::from(tree.key_length)
                };
                page = u32_be_at(&node, NODE_HEADER_SIZE + key_length + 4);
                continue;
            }
            if tree.binary {
                decode_bitmap(&node, tree, &mut keys)?;
            } else {
                decode_leaf(&node, tree, &mut keys)?;
            }
            page = u32_at(&node, 8);
            if page == NONE {
                return Ok(keys);
            }
        }
    }
}

/// The deepest tree a cursor descends; deeper is taken as a loop in a
/// corrupt file. (A tree of 512-byte pages with the shortest keys holds
/// billions of keys in a dozen levels.)
const MAX_DEPTH: usize = 32;

/// A leaf, decoded.
struct Leaf {
    keys: Vec<Key>,
    left: u32,
    right: u32,
}

impl IndexFile {
    /// Reads page `page` of a tree, checking that it is in the file.
    fn node(&mut self, page: u32) -> Result<Vec<u8>, Error> {
        if page == NONE || u64::from(page) >= self.pages()? {
            return Err(Error::Corrupt("tree page out of the file"));
        }
        self.page(page)
    }

    /// The pages of the file, the most a walk through a tree may visit.
    fn pages(&self) -> Result<u64, Error> {
        Ok(self.file.metadata()?.len() / u64::from(self.page_size))
    }

    /// The leaf at `page`, decoded.
    fn leaf(&mut self, tree: &TreeRef, page: u32) -> Result<Leaf, Error> {
        let node = self.node(page)?;
        if !is_leaf(&node) {
            return Err(Error::Corrupt("expected a leaf"));
        }
        let mut keys = Vec::new();
        if tree.binary {
            decode_bitmap(&node, tree, &mut keys)?;
        } else {
            decode_leaf(&node, tree, &mut keys)?;
        }
        Ok(Leaf {
            keys,
            left: u32_at(&node, 4),
            right: u32_at(&node, 8),
        })
    }

    /// The number of keys in the leaf at `page`, and its sibling in
    /// direction `right`. A compact leaf's header counts them; a bitmap is
    /// decoded.
    fn leaf_count(
        &mut self,
        tree: &TreeRef,
        page: u32,
        right: bool,
    ) -> Result<(usize, u32), Error> {
        let node = self.node(page)?;
        if !is_leaf(&node) {
            return Err(Error::Corrupt("expected a leaf"));
        }
        let count = if tree.binary {
            let mut keys = Vec::new();
            decode_bitmap(&node, tree, &mut keys)?;
            keys.len()
        } else {
            usize::from(u16_at(&node, 2))
        };
        Ok((count, u32_at(&node, if right { 8 } else { 4 })))
    }

    /// The entries of the interior node `node`: the last key of each child
    /// subtree (with its record) and the child's page. Binary orders store
    /// no key bytes in their interior nodes; their entries get the key every
    /// record of theirs has, the pad byte.
    fn interior(tree: &TreeRef, node: &[u8]) -> Result<Vec<(Key, u32)>, Error> {
        let count = usize::from(u16_at(node, 2));
        let key_length = if tree.binary {
            0
        } else {
            usize::from(tree.key_length)
        };
        let entry = key_length + 8;
        if count == 0 || NODE_HEADER_SIZE + count * entry > node.len() {
            return Err(Error::Corrupt("invalid interior node"));
        }
        Ok((0..count)
            .map(|i| {
                let at = NODE_HEADER_SIZE + i * entry;
                let bytes = match tree.binary {
                    true => vec![tree.pad; usize::from(tree.key_length)],
                    false => node[at..at + key_length].to_vec(),
                };
                let record = u32_be_at(node, at + key_length);
                (Key { bytes, record }, u32_be_at(node, at + key_length + 4))
            })
            .collect())
    }

    /// The leaf a descent from the root reaches, taking at each interior
    /// node the child `choose` picks from its entries (`None`: past the
    /// last).
    fn descend(
        &mut self,
        tree: &TreeRef,
        choose: impl Fn(&[(Key, u32)]) -> Option<usize>,
    ) -> Result<Option<u32>, Error> {
        let mut page = tree.root;
        for _ in 0..MAX_DEPTH {
            let node = self.node(page)?;
            if is_leaf(&node) {
                return Ok(Some(page));
            }
            let entries = IndexFile::interior(tree, &node)?;
            match choose(&entries) {
                Some(i) => page = entries[i].1,
                None => return Ok(None),
            }
        }
        Err(Error::Corrupt("tree too deep"))
    }
}

/// Whether `node` is a leaf (attribute bit 0x02).
fn is_leaf(node: &[u8]) -> bool {
    u16_at(node, 0) & 0x02 != 0
}

/// How far a leaf page displaces bytes to fit the trailing IV: the gap just
/// after its entry array (`end`), and the start of the bytes it moves there
/// (`start`, at the IV region or the end of the free gap, whichever is later).
/// `None` for an interior page or when the layout leaves no room.
fn aes_leaf_shift(page: &[u8]) -> Option<(usize, usize)> {
    if !is_leaf(page) {
        return None;
    }
    let size = page.len();
    let split = size - 8;
    let count = usize::from(u16_at(page, 2));
    let entry_size = usize::from(page[23]);
    let free = usize::from(u16_at(page, 12));
    let end = LEAF_HEADER_SIZE + count * entry_size;
    let start = split.max(end + free);
    (end + 8 <= size && start <= size).then_some((end, start))
}

/// Decipher an AES index tree page in place (the raw page, its trailing IV
/// included). The page is `[data][8-byte IV]`: decipher the data under the IV,
/// then, for a leaf, put back the up-to-8 bytes it displaced to the gap after
/// its entry array to make room for the IV. The inverse of [`aes_encipher_page`];
/// used on read (see [`IndexFile::page`]) and index maintenance.
pub(crate) fn aes_decipher_page(key: &[u8], nonce: &[u8; 4], bytes: &mut [u8]) {
    let size = bytes.len();
    let split = size - 8;
    let iv: [u8; 8] = bytes[split..].try_into().unwrap();
    crate::crypto::aes_ctr_xor(key, &iv, nonce, &mut bytes[..split]);
    bytes[split..].fill(0);
    if let Some((end, start)) = aes_leaf_shift(bytes) {
        let moved: [u8; 8] = bytes[end..end + 8].try_into().unwrap();
        bytes[end..end + 8].fill(0);
        bytes[start..].copy_from_slice(&moved[..size - start]);
    }
}

/// Encipher a clear AES index tree page in place, writing `iv` into its last 8
/// bytes: the inverse of [`aes_decipher_page`]. For a leaf, the up-to-8 bytes
/// where the IV will go first move into the free gap after its entry array.
pub(crate) fn aes_encipher_page(key: &[u8], nonce: &[u8; 4], iv: &[u8; 8], bytes: &mut [u8]) {
    let size = bytes.len();
    let split = size - 8;
    if let Some((end, start)) = aes_leaf_shift(bytes) {
        let moved = bytes[start..].to_vec();
        bytes[end..end + moved.len()].copy_from_slice(&moved);
    }
    bytes[split..].copy_from_slice(iv);
    crate::crypto::aes_ctr_xor(key, iv, nonce, &mut bytes[..split]);
}

impl IndexFile {
    /// The pages that stay in the clear in an encrypted index: the orders'
    /// tag headers (a header is [`HEADER_SIZE`] bytes, more than a page of
    /// 512) and the options blocks of the full-text orders
    /// (`fixtures/aes/dictwrite`, `40_fts_create`).
    fn header_pages(&mut self) -> std::collections::HashSet<u32> {
        let mut pages: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for tag in self.tags.clone() {
            pages.extend(self.clear_pages_of(&tag));
        }
        pages
    }

    /// The pages of the order `tag` that stay in the clear: its tag header
    /// and, if it is full-text, its options block.
    fn clear_pages_of(&mut self, tag: &Tag) -> Vec<u32> {
        let header = (HEADER_SIZE as u32).div_ceil(self.page_size);
        let mut pages: Vec<u32> = (tag.header_page..tag.header_page + header).collect();
        if tag.full_text() {
            // The block's length, read as it is stored.
            let mut length = [0u8; 4];
            let at = u64::from(tag.fts_block) * u64::from(self.page_size) + 4;
            let read = self
                .file
                .seek(SeekFrom::Start(at))
                .and_then(|_| self.file.read_exact(&mut length));
            let count = match read {
                Ok(()) => u32::from_le_bytes(length).div_ceil(self.page_size).max(1),
                Err(_) => 1,
            };
            pages.extend(tag.fts_block..tag.fts_block + count);
        }
        pages
    }
}

/// Whether `page` of an encrypted index was never enciphered: it holds
/// nothing, or has no IV behind its data, as the pages an order that was
/// dropped left do (`fixtures/aes/dictwrite`, `40_fts_create`).
fn is_clear_page(page: &[u8]) -> bool {
    page.iter().all(|&b| b == 0) || page[page.len() - 8..].iter().all(|&b| b == 0)
}

/// The pages of the encrypted index at `path` that hold something in the
/// clear and belong to no order: what a dropped order left, which stays as
/// it is when the file is enciphered again.
fn clear_orphans(
    path: &Path,
    key: &[u8],
    nonce: &[u8; 4],
) -> Result<std::collections::HashSet<u32>, Error> {
    let cipher = RecordCipher::Aes {
        key: key.to_vec(),
        nonce: *nonce,
    };
    let mut index = IndexFile::open_encrypted(path, &cipher)?;
    let size = index.page_size as usize;
    let headers = index.header_pages();
    drop(index);
    let bytes = std::fs::read(path)?;
    Ok(bytes
        .chunks(size)
        .enumerate()
        .skip(1)
        .filter(|(number, page)| {
            page.len() == size
                && !headers.contains(&(*number as u32))
                && page.iter().any(|&b| b != 0)
                && is_clear_page(page)
        })
        .map(|(number, _)| number as u32)
        .collect())
}

/// The bytes of the AES-encrypted index at `path` with its tree pages
/// deciphered (with `key` and `nonce`) and their IVs zeroed: what tells two
/// such indexes apart beyond their random IVs, as the tests compare them.
pub fn deciphered(path: &Path, key: &[u8], nonce: &[u8; 4]) -> Result<Vec<u8>, Error> {
    let cipher = RecordCipher::Aes {
        key: key.to_vec(),
        nonce: *nonce,
    };
    let mut index = IndexFile::open_encrypted(path, &cipher)?;
    let size = index.page_size as usize;
    let headers = index.header_pages();
    drop(index);
    let mut bytes = std::fs::read(path)?;
    for (number, page) in bytes.chunks_mut(size).enumerate().skip(1) {
        if page.len() == size && !headers.contains(&(number as u32)) && !is_clear_page(page) {
            aes_decipher_page(key, nonce, page);
        }
    }
    Ok(bytes)
}

/// Marks the ADI file at `path` as an AES-encrypted table's index, as the
/// original marks one (`fixtures/aes/dictwrite`): its feature level 5 (at
/// 0x08), the flags word in use (0x0E) holding [`flags::ENCRYPTED`] (0x18),
/// and the table's 20-byte `verifier` (0x20). Its orders are built before,
/// with each page's IV room free (`crate::create::build_order_reserving`),
/// and its tree pages enciphered ([`encipher_tree_pages`]), which reads the
/// file as one in the clear.
pub fn make_aes(path: &Path, verifier: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut header = [0u8; 0x34];
    file.read_exact(&mut header)?;
    header[0x08..0x0C].copy_from_slice(&5u32.to_le_bytes());
    header[0x0E] |= options::FLAGS;
    let marks = u32_at(&header, 0x18) | flags::ENCRYPTED;
    header[0x18..0x1C].copy_from_slice(&marks.to_le_bytes());
    header[0x20..0x34].copy_from_slice(&verifier[..20]);
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&header)
}

/// Enciphers the ADI file at `path`, built in the clear, as the original
/// keeps the index of a dictionary that encrypts its records with RC4
/// (`fixtures/aes/dictwrite`, `22_rc4_create`): each tree page one cipher
/// stream under `key` at its page number, the file header (feature level 5, the
/// flags word in use and [`flags::ENCRYPTED`]) holding the password check at
/// 0x20, the word `advantage` enciphered at item 0x2D. The orders' tag headers
/// stay clear, and a page nothing was written to.
pub fn encipher_rc4(path: &Path, key: &[u8]) -> Result<(), Error> {
    use std::io::Write;
    let mut index = IndexFile::open(path)?;
    let size = index.page_size as usize;
    let headers = index.header_pages();
    let pages = index.pages()?;
    drop(index);
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut page = vec![0u8; size];
    for number in 1..pages as u32 {
        if headers.contains(&number) {
            continue;
        }
        let at = u64::from(number) * size as u64;
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(&mut page)?;
        if page.iter().all(|&b| b == 0) {
            continue;
        }
        crate::crypto::rc4_crypt(key, number, &mut page);
        file.seek(SeekFrom::Start(at))?;
        file.write_all(&page)?;
    }
    let mut header = [0u8; 0x34];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header)?;
    header[0x08..0x0C].copy_from_slice(&5u32.to_le_bytes());
    header[0x0E] |= options::FLAGS;
    let marks = u32_at(&header, 0x18) | flags::ENCRYPTED;
    header[0x18..0x1C].copy_from_slice(&marks.to_le_bytes());
    let mut check = [0u8; 20];
    check[..9].copy_from_slice(b"advantage");
    crate::crypto::rc4_crypt(key, 0x2D, &mut check);
    header[0x20..0x34].copy_from_slice(&check);
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&header)?;
    Ok(())
}

/// Takes the RC4 encryption of [`encipher_rc4`] off the ADI file at `path`:
/// its tree pages put back in the clear and its header marks cleared, as
/// [`decipher_in_place`] does an AES one's.
pub fn decipher_rc4(path: &Path, key: &[u8]) -> Result<(), Error> {
    use std::io::Write;
    let cipher = RecordCipher::Rc4(key.to_vec());
    let mut index = IndexFile::open_encrypted(path, &cipher)?;
    let size = index.page_size as usize;
    let headers = index.header_pages();
    drop(index);
    let mut bytes = std::fs::read(path)?;
    for (number, page) in bytes.chunks_mut(size).enumerate().skip(1) {
        if page.len() == size && !headers.contains(&(number as u32)) && page.iter().any(|&b| b != 0)
        {
            crate::crypto::rc4_crypt(key, number as u32, page);
        }
    }
    bytes[0x08..0x0C].copy_from_slice(&1u32.to_le_bytes());
    bytes[0x0E] &= !options::FLAGS;
    let marks = u32_at(&bytes, 0x18) & !flags::ENCRYPTED;
    bytes[0x18..0x1C].copy_from_slice(&marks.to_le_bytes());
    bytes[0x20..0x34].fill(0);
    let mut file = std::fs::OpenOptions::new().write(true).open(path)?;
    file.write_all(&bytes)?;
    Ok(())
}

/// Takes the AES encryption off the ADI file at `path`, as the original does
/// when its table is decrypted (`fixtures/aes/dictwrite`, `15_decrypt_all`):
/// its tree pages are put back in the clear ([`deciphered`]) and its header
/// marks, which [`make_aes`] set, cleared.
pub fn decipher_in_place(path: &Path, key: &[u8], nonce: &[u8; 4]) -> Result<(), Error> {
    let mut bytes = deciphered(path, key, nonce)?;
    bytes[0x08..0x0C].copy_from_slice(&1u32.to_le_bytes());
    bytes[0x0E] &= !options::FLAGS;
    let marks = u32_at(&bytes, 0x18) & !flags::ENCRYPTED;
    bytes[0x18..0x1C].copy_from_slice(&marks.to_le_bytes());
    bytes[0x20..0x34].fill(0);
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Whether the ADI file at `path` is an AES-encrypted table's index, as
/// [`make_aes`] marks one.
pub fn is_aes(path: &Path) -> bool {
    let mut header = [0u8; 0x1C];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok_and(|()| u32_at(&header, 0x18) & flags::ENCRYPTED != 0)
}

/// Runs `work` on the ADI file at `path` in the clear, if it is an
/// AES-encrypted table's index, and enciphers it again after, whatever
/// `work` returns (`unreadable` makes the error of a file that cannot be
/// read or written): the writers that make, drop and rebuild orders only know
/// plain pages. A file `work` makes where there was none is the first index
/// of an encrypted table, enciphered as the original enciphers it
/// (`fixtures/aes/dictwrite`, `20_index_first`). Each page takes a fresh IV;
/// the file's verifier is the table's, `verifier`.
pub fn with_clear<T, E>(
    path: &Path,
    key: &[u8],
    nonce: &[u8; 4],
    counter: &std::sync::atomic::AtomicU64,
    verifier: &[u8],
    work: impl FnOnce() -> Result<T, E>,
    unreadable: impl Fn() -> E,
) -> Result<T, E> {
    if !path.exists() {
        let done = work();
        if path.exists() {
            encipher_tree_pages(path, key, nonce, counter).map_err(|_| unreadable())?;
            make_aes(path, verifier).map_err(|_| unreadable())?;
        }
        return done;
    }
    if !is_aes(path) {
        return work();
    }
    let orphans = clear_orphans(path, key, nonce).map_err(|_| unreadable())?;
    let enciphered = std::fs::read(path).map_err(|_| unreadable())?;
    decipher_in_place(path, key, nonce).map_err(|_| unreadable())?;
    let plain = std::fs::read(path).map_err(|_| unreadable())?;
    let mut before = IndexFile::open(path).map_err(|_| unreadable())?;
    let size = before.page_size as usize;
    // What stays in the clear of each order, taken before the work changes
    // the file.
    let clear: Vec<(String, Vec<u32>)> = before
        .tags
        .clone()
        .iter()
        .map(|tag| (tag.name.clone(), before.clear_pages_of(tag)))
        .collect();
    drop(before);
    let done = work();
    // A file the work removed has nothing to encipher.
    if path.exists() {
        // The pages of an order the work dropped stay as they were, its tag
        // header and options block in the clear, where they are no tree
        // pages.
        let after = IndexFile::open(path).map_err(|_| unreadable())?;
        let dropped: std::collections::HashSet<u32> = clear
            .iter()
            .filter(|(name, _)| !after.tags.iter().any(|t| &t.name == name))
            .flat_map(|(_, pages)| pages.iter().copied())
            .chain(orphans)
            .collect();
        drop(after);
        // The original enciphers the pages it writes and no others: a page
        // the work left as it was keeps its bytes and its IV
        // (`fixtures/aes/dictwrite`, `16_index_create`).
        let now = std::fs::read(path).map_err(|_| unreadable())?;
        let unchanged: Vec<u32> = (1..(plain.len().min(now.len()) / size) as u32)
            .filter(|&number| {
                let page = |bytes: &[u8]| bytes[number as usize * size..][..size].to_vec();
                page(&plain) == page(&now)
            })
            .collect();
        let skip: std::collections::HashSet<u32> =
            dropped.iter().chain(&unchanged).copied().collect();
        // The pages are read as plain, which a file marked encrypted is not.
        encipher_pages(path, key, nonce, counter, &skip).map_err(|_| unreadable())?;
        make_aes(path, verifier).map_err(|_| unreadable())?;
        restore_pages(path, &enciphered, size, &unchanged).map_err(|_| unreadable())?;
    }
    done
}

/// [`with_clear`] for the index of a table of a dictionary of RC4 tables
/// (`fixtures/aes/rc4tp`): the file is put in the clear if it is
/// enciphered, and enciphered again after `work`; a file `work` makes where
/// there was none is enciphered when the dictionary encrypts indexes
/// (`encrypt_new`), and one that is in the clear stays so.
pub fn with_clear_rc4<T, E>(
    path: &Path,
    key: &[u8],
    encrypt_new: bool,
    work: impl FnOnce() -> Result<T, E>,
    unreadable: impl Fn() -> E,
) -> Result<T, E> {
    let key = &key[..key.len().min(crate::crypto::KEY_MAX)];
    if !path.exists() {
        let done = work();
        if encrypt_new && path.exists() {
            encipher_rc4(path, key).map_err(|_| unreadable())?;
        }
        return done;
    }
    if !is_aes(path) {
        return work();
    }
    decipher_rc4(path, key).map_err(|_| unreadable())?;
    let done = work();
    if path.exists() {
        encipher_rc4(path, key).map_err(|_| unreadable())?;
    }
    done
}

/// Writes the pages `numbers` of the index at `path` as they are in `bytes`,
/// a copy of the file that was.
fn restore_pages(
    path: &Path,
    bytes: &[u8],
    size: usize,
    numbers: &[u32],
) -> Result<(), std::io::Error> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().write(true).open(path)?;
    for &number in numbers {
        let at = number as usize * size;
        if at + size <= bytes.len() {
            file.seek(SeekFrom::Start(at as u64))?;
            file.write_all(&bytes[at..at + size])?;
        }
    }
    Ok(())
}

/// Enciphers the tree pages of the ADI file at `path`, built in the clear
/// (the directory's and each order's; the file header and the orders' tag
/// headers stay clear), with `key` and `nonce`, each under a fresh IV from
/// `counter`, as an AES-encrypted table's index keeps them. Every page but
/// those headers must be a tree page or empty, as in a file built anew.
pub fn encipher_tree_pages(
    path: &Path,
    key: &[u8],
    nonce: &[u8; 4],
    counter: &std::sync::atomic::AtomicU64,
) -> Result<(), Error> {
    encipher_pages(path, key, nonce, counter, &Default::default())
}

/// [`encipher_tree_pages`], leaving the pages in `skip` as they are.
fn encipher_pages(
    path: &Path,
    key: &[u8],
    nonce: &[u8; 4],
    counter: &std::sync::atomic::AtomicU64,
    skip: &std::collections::HashSet<u32>,
) -> Result<(), Error> {
    use std::io::Write;
    let mut index = IndexFile::open(path)?;
    let size = index.page_size as usize;
    let headers = index.header_pages();
    let pages = index.pages()?;
    drop(index);
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut page = vec![0u8; size];
    // Page 0 is the file header.
    for number in 1..pages as u32 {
        if headers.contains(&number) || skip.contains(&number) {
            continue;
        }
        let at = u64::from(number) * size as u64;
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(&mut page)?;
        // A page nothing was written to stays as it is.
        if page.iter().all(|&b| b == 0) {
            continue;
        }
        let iv = counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_le_bytes();
        aes_encipher_page(key, nonce, &iv, &mut page);
        file.seek(SeekFrom::Start(at))?;
        file.write_all(&page)?;
    }
    Ok(())
}

/// How `entry` compares with a sought `key` and `record`: by the first
/// `key.len()` bytes of its key, then, if `record` is given, by its record
/// number. Keys order as bytes; equal keys by record number.
fn compare(entry: &Key, key: &[u8], record: Option<u32>) -> Ordering {
    let length = key.len().min(entry.bytes.len());
    entry.bytes[..length]
        .cmp(&key[..length])
        .then_with(|| record.map_or(Ordering::Equal, |record| entry.record.cmp(&record)))
}

/// Where a cursor is, in the order the keys are stored (ascending).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Place {
    /// Before the first stored key.
    Start,
    /// On key `slot` of the leaf at `page`.
    On { page: u32, slot: usize },
    /// After the last stored key.
    End,
}

/// A position in one order of an index file, moving through its keys in
/// index order (ascending, or reversed for a descending order, as
/// [`IndexFile::keys`] lists them) without reading the whole order.
///
/// A cursor is before the first key, on a key, or after the last key; it
/// starts before the first. It holds the leaf it is on, decoded; every move
/// reads the pages it needs from the file, so the cursor itself never holds
/// more than one leaf. The file is passed to each call, so that one open
/// file can serve the cursors of all its orders.
#[derive(Debug, Clone)]
pub struct Cursor {
    tree: TreeRef,
    descending: bool,
    place: Place,
    /// The keys of the leaf at `place`, if it is on one.
    keys: Vec<Key>,
    left: u32,
    right: u32,
}

impl Cursor {
    /// A cursor before the first key of `tag`.
    pub fn new(tag: &Tag) -> Cursor {
        Cursor {
            tree: TreeRef::of(tag),
            descending: tag.descending,
            place: if tag.descending {
                Place::End
            } else {
                Place::Start
            },
            keys: Vec::new(),
            left: NONE,
            right: NONE,
        }
    }

    /// The key the cursor is on, if any.
    pub fn key(&self) -> Option<&Key> {
        match self.place {
            Place::On { slot, .. } => self.keys.get(slot),
            _ => None,
        }
    }

    /// Whether the cursor is before the first key (in index order).
    pub fn before_first(&self) -> bool {
        self.place
            == if self.descending {
                Place::End
            } else {
                Place::Start
            }
    }

    /// Whether the cursor is after the last key (in index order).
    pub fn after_last(&self) -> bool {
        self.place
            == if self.descending {
                Place::Start
            } else {
                Place::End
            }
    }

    /// Moves to the first key, or after the last if the order has none.
    pub fn first(&mut self, index: &mut IndexFile) -> Result<Option<&Key>, Error> {
        match self.descending {
            false => self.stored_first(index)?,
            true => self.stored_last(index)?,
        }
        Ok(self.key())
    }

    /// Moves to the last key, or before the first if the order has none.
    pub fn last(&mut self, index: &mut IndexFile) -> Result<Option<&Key>, Error> {
        match self.descending {
            false => self.stored_last(index)?,
            true => self.stored_first(index)?,
        }
        Ok(self.key())
    }

    /// Moves to the next key, or after the last. From before the first key
    /// that is the first key; after the last it stays there.
    pub fn next(&mut self, index: &mut IndexFile) -> Result<Option<&Key>, Error> {
        self.step(index, !self.descending)?;
        Ok(self.key())
    }

    /// Moves to the previous key, or before the first. From after the last
    /// key that is the last key; before the first it stays there.
    pub fn prev(&mut self, index: &mut IndexFile) -> Result<Option<&Key>, Error> {
        self.step(index, self.descending)?;
        Ok(self.key())
    }

    /// Moves to the first key, in index order, that is not before `key`
    /// and `record`, or after the last key if there is none. Keys compare by
    /// their first `key.len()` bytes, so a shorter `key` finds the first key
    /// it begins (or the first after where it would be). With `record`, an
    /// entry equal in those bytes compares by its record number, so a full
    /// key and its record find exactly that entry.
    ///
    /// In an ascending order that is the first key at or above `key`; in a
    /// descending order the first, in index order, at or below it.
    pub fn seek(
        &mut self,
        index: &mut IndexFile,
        key: &[u8],
        record: Option<u32>,
    ) -> Result<Option<&Key>, Error> {
        if self.descending {
            // The last stored entry at or below: the one before the first
            // above.
            self.stored_bound(index, |entry| {
                compare(entry, key, record) == Ordering::Greater
            })?;
            self.step(index, false)?;
        } else {
            self.stored_bound(index, |entry| compare(entry, key, record) != Ordering::Less)?;
        }
        Ok(self.key())
    }

    /// The keys of the leaf the cursor is on that follow its key in the stored
    /// order (`forward`) or precede it, the cursor's own included, nearest
    /// first: what a reader that takes a leaf at a time has left of it. And
    /// whether the leaf has a sibling on that side, as it was read.
    pub fn leaf_keys(&self, forward: bool) -> (Vec<Key>, bool) {
        let Place::On { slot, .. } = self.place else {
            return (Vec::new(), false);
        };
        match forward {
            true => (self.keys[slot..].to_vec(), self.right != NONE),
            false => (
                self.keys[..=slot].iter().rev().cloned().collect(),
                self.left != NONE,
            ),
        }
    }

    /// The place of the current key in index order, 1-based; `None` before
    /// the first key or after the last. Counts the keys of every leaf before
    /// the current one, so it reads those leaves.
    pub fn key_number(&self, index: &mut IndexFile) -> Result<Option<u32>, Error> {
        let Place::On { slot, .. } = self.place else {
            return Ok(None);
        };
        let (mut count, mut page) = match self.descending {
            false => (slot, self.left),
            true => (self.keys.len() - 1 - slot, self.right),
        };
        let mut visited = 0;
        while page != NONE {
            visited += 1;
            if visited > index.pages()? {
                return Err(Error::Corrupt("leaf siblings loop"));
            }
            let (keys, next) = index.leaf_count(&self.tree, page, self.descending)?;
            count += keys;
            page = next;
        }
        Ok(Some(count as u32 + 1))
    }

    /// The number of keys in the order. Reads every leaf.
    pub fn key_count(&self, index: &mut IndexFile) -> Result<u32, Error> {
        let Some(mut page) = index.descend(&self.tree, |_| Some(0))? else {
            return Ok(0);
        };
        let mut count = 0;
        let mut visited = 0;
        while page != NONE {
            visited += 1;
            if visited > index.pages()? {
                return Err(Error::Corrupt("leaf siblings loop"));
            }
            let (keys, next) = index.leaf_count(&self.tree, page, true)?;
            count += keys;
            page = next;
        }
        Ok(count as u32)
    }

    /// Puts the cursor on `slot` of the leaf at `page`, decoded as `leaf`.
    fn land(&mut self, page: u32, leaf: Leaf, slot: usize) {
        self.keys = leaf.keys;
        self.left = leaf.left;
        self.right = leaf.right;
        self.place = Place::On { page, slot };
    }

    /// Moves off the keys, to `place` ([`Place::Start`] or [`Place::End`]).
    fn leave(&mut self, place: Place) {
        self.keys.clear();
        self.left = NONE;
        self.right = NONE;
        self.place = place;
    }

    /// Moves to the first stored key, or to the end if there is none.
    fn stored_first(&mut self, index: &mut IndexFile) -> Result<(), Error> {
        self.leave(Place::Start);
        self.step(index, true)
    }

    /// Moves to the last stored key, or to the start if there is none.
    fn stored_last(&mut self, index: &mut IndexFile) -> Result<(), Error> {
        self.leave(Place::End);
        self.step(index, false)
    }

    /// Moves one stored key forward or back, past empty leaves; off the
    /// last key to the end, off the first to the start.
    fn step(&mut self, index: &mut IndexFile, forward: bool) -> Result<(), Error> {
        let page = match (&self.place, forward) {
            (Place::End, true) | (Place::Start, false) => return Ok(()),
            (Place::Start, true) => index.descend(&self.tree, |_| Some(0))?.unwrap_or(NONE),
            (Place::End, false) => {
                let last = index.descend(&self.tree, |entries| Some(entries.len() - 1))?;
                last.unwrap_or(NONE)
            }
            (&Place::On { page, slot }, true) if slot + 1 < self.keys.len() => {
                self.place = Place::On {
                    page,
                    slot: slot + 1,
                };
                return Ok(());
            }
            (&Place::On { page, slot }, false) if slot > 0 => {
                self.place = Place::On {
                    page,
                    slot: slot - 1,
                };
                return Ok(());
            }
            (Place::On { .. }, true) => self.right,
            (Place::On { .. }, false) => self.left,
        };
        self.walk_from(index, page, forward)
    }

    /// Moves to the first key (`forward`) or the last key of the leaf at
    /// `page`, or of the first leaf in that direction from it that has keys;
    /// past them all to the end or the start.
    fn walk_from(
        &mut self,
        index: &mut IndexFile,
        mut page: u32,
        forward: bool,
    ) -> Result<(), Error> {
        let mut visited = 0;
        while page != NONE {
            visited += 1;
            if visited > index.pages()? {
                return Err(Error::Corrupt("leaf siblings loop"));
            }
            let leaf = index.leaf(&self.tree, page)?;
            if !leaf.keys.is_empty() {
                let slot = if forward { 0 } else { leaf.keys.len() - 1 };
                self.land(page, leaf, slot);
                return Ok(());
            }
            page = if forward { leaf.right } else { leaf.left };
        }
        self.leave(if forward { Place::End } else { Place::Start });
        Ok(())
    }

    /// Moves to the first stored key for which `reached` holds, or to the
    /// end if there is none; `reached` must be false up to some key and
    /// true from there on.
    fn stored_bound(
        &mut self,
        index: &mut IndexFile,
        reached: impl Fn(&Key) -> bool,
    ) -> Result<(), Error> {
        // An interior entry holds the last key of its child, so the first
        // child whose last key is reached holds the first key reached.
        let page = index.descend(&self.tree, |entries| {
            entries.iter().position(|(key, _)| reached(key))
        })?;
        let Some(page) = page else {
            self.leave(Place::End);
            return Ok(());
        };
        let leaf = index.leaf(&self.tree, page)?;
        match leaf.keys.iter().position(&reached) {
            Some(slot) => {
                self.land(page, leaf, slot);
                Ok(())
            }
            // Only the root leaf of an empty order has no key reached.
            None => self.walk_from(index, leaf.right, true),
        }
    }
}

/// What walking a tree needs to know.
#[derive(Debug, Clone)]
pub(crate) struct TreeRef {
    pub(crate) root: u32,
    pub(crate) key_length: u16,
    pub(crate) pad: u8,
    /// A binary order, whose leaves are bitmaps.
    pub(crate) binary: bool,
}

impl TreeRef {
    pub(crate) fn of(tag: &Tag) -> TreeRef {
        TreeRef {
            root: tag.root,
            key_length: tag.key_length,
            pad: tag.pad,
            binary: tag.binary(),
        }
    }
}

/// The offset of a bitmap leaf's bits, after the node header and the record
/// number of the first bit.
const BITMAP_OFFSET: usize = 16;

/// Appends the keys of the bitmap leaf `node` to `keys`: one per record whose
/// bit is clear. Bit *i* (least significant first) is record *first* + *i*;
/// a set bit marks a deleted record, and record 0 does not exist.
fn decode_bitmap(node: &[u8], tree: &TreeRef, keys: &mut Vec<Key>) -> Result<(), Error> {
    let bits = usize::from(u16_at(node, 2));
    let first = u32_be_at(node, NODE_HEADER_SIZE);
    if bits > (node.len() - BITMAP_OFFSET) * 8 {
        return Err(Error::Corrupt("bitmap overflows the page"));
    }
    for i in 0..bits {
        let deleted = node[BITMAP_OFFSET + i / 8] & (1 << (i % 8)) != 0;
        let record = first + i as u32;
        if record != 0 && !deleted {
            keys.push(Key {
                bytes: vec![tree.pad; usize::from(tree.key_length)],
                record,
            });
        }
    }
    Ok(())
}

/// Appends the keys of the compact leaf `node` to `keys`.
///
/// Each entry is an integer holding the record number, the count of bytes
/// shared with the previous key and the count of pad bytes trailing the key;
/// the remaining bytes of each key are packed from the end of the page
/// backwards.
pub(crate) fn decode_leaf(node: &[u8], tree: &TreeRef, keys: &mut Vec<Key>) -> Result<(), Error> {
    let count = usize::from(u16_at(node, 2));
    let record_bits = u32::from(node[20]);
    let dup_bits = u32::from(node[21]);
    let trail_bits = u32::from(node[22]);
    let entry_size = usize::from(node[23]);
    let key_length = usize::from(tree.key_length);
    if count > 0 && (entry_size == 0 || entry_size > 8 || record_bits + dup_bits + trail_bits > 64)
    {
        return Err(Error::Corrupt("invalid leaf layout"));
    }
    let mask = |bits: u32| {
        if bits >= 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        }
    };
    let mut previous: Vec<u8> = Vec::new();
    let mut end = node.len();
    for i in 0..count {
        let start = LEAF_HEADER_SIZE + i * entry_size;
        let entry = node
            .get(start..start + entry_size)
            .ok_or(Error::Corrupt("leaf overflows"))?;
        let mut value = 0u64;
        for (shift, &byte) in entry.iter().enumerate() {
            value |= u64::from(byte) << (8 * shift);
        }
        let record = (value & mask(record_bits)) as u32;
        let dup = ((value >> record_bits) & mask(dup_bits)) as usize;
        let trail = ((value >> (record_bits + dup_bits)) & mask(trail_bits)) as usize;
        let stored = key_length
            .checked_sub(dup + trail)
            .filter(|_| dup <= previous.len())
            .ok_or(Error::Corrupt("leaf entry overflows its key"))?;
        end = end
            .checked_sub(stored)
            .filter(|&end| end >= start + entry_size)
            .ok_or(Error::Corrupt("leaf keys overflow the page"))?;
        let mut key = previous[..dup].to_vec();
        key.extend_from_slice(&node[end..end + stored]);
        key.resize(key_length, tree.pad);
        keys.push(Key {
            bytes: key.clone(),
            record,
        });
        previous = key;
    }
    Ok(())
}

/// The parsing options of the full-text order `tag` from its stored options
/// block.
pub(crate) fn fts_options_of(tag: &Tag, block: crate::create::FtsBlock) -> FtsOptions {
    FtsOptions {
        field: tag.expression.clone(),
        min_word: block.min_word,
        max_word: block.max_word,
        // The full-text bit is implied, not stored.
        options: block.options | FTS_INDEX_BIT,
        delimiters: cp1252::decode(&block.delimiters),
        noise: cp1252::decode(&block.noise),
        drop: cp1252::decode(&block.drop),
        conditional: cp1252::decode(&block.conditional),
    }
}

/// The options block of a full-text order from the bytes it is stored in
/// (see [`crate::create::FtsBlock`]), however the file addresses them.
pub(crate) fn parse_fts_block(bytes: &[u8]) -> Result<crate::create::FtsBlock, Error> {
    let at = |o: usize| -> Result<u32, Error> {
        bytes
            .get(o..o + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .ok_or(Error::Corrupt("full-text options run past the block"))
    };
    let mut offset = 0x14;
    let mut list = || -> Result<Vec<u8>, Error> {
        let length = at(offset)? as usize;
        offset += 4;
        let text = bytes
            .get(offset..offset + length)
            .ok_or(Error::Corrupt("full-text option past the block"))?;
        offset += length;
        Ok(trim_nul(text).to_vec())
    };
    let (delimiters, noise, drop, conditional) = (list()?, list()?, list()?, list()?);
    Ok(crate::create::FtsBlock {
        options: at(0x08)?,
        min_word: at(0x0C)?,
        max_word: at(0x10)?,
        delimiters,
        noise,
        drop,
        conditional,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::Value as Json;

    use super::*;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt")
    }

    /// Full-text order parsing options read from the `.adi` match what
    /// `AdsGetFTSIndexInfo` reports for the fixtures (default and custom).
    #[test]
    fn fts_options_match_the_engine() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sql/fts");
        let options = |file: &str, tag: &str| {
            let mut index = IndexFile::open(&dir.join(file)).unwrap();
            let tag = index.tags.iter().find(|t| t.name == tag).unwrap().clone();
            index.fts_options(&tag).unwrap().unwrap()
        };
        let default_delims = "\x08\t\n\x0b\x0c\r ";
        let default_drop = "'\"`";
        let default_cond = ",.?!;:@#$%^&()-_";

        // docs: the default full-text index.
        let docs = options("docs.adi", "BODY_FTS");
        assert_eq!(
            (docs.field.as_str(), docs.min_word, docs.max_word),
            ("F3", 3, 30)
        );
        assert_eq!(docs.options, 0x20);
        assert_eq!(docs.delimiters, default_delims);
        assert_eq!(docs.drop, default_drop);
        assert_eq!(docs.conditional, default_cond);
        assert!(docs.noise.starts_with("about after all"));
        assert!(docs.noise.ends_with("your"));

        // opts C_FTS: case-sensitive, keep-score, a 2..5 word window, custom noise.
        let c = options("opts.adi", "C_FTS");
        assert_eq!(
            (c.field.as_str(), c.min_word, c.max_word, c.options),
            ("F2", 2, 5, 0x1a0)
        );
        assert!(c.noise.split(' ').any(|w| w == "an")); // a bigger noise list

        // opts M_FTS: protect-numbers, custom delimiters/drop/conditional/noise.
        let m = options("opts.adi", "M_FTS");
        assert_eq!((m.field.as_str(), m.options), ("F3", 0x220));
        assert_eq!(
            (
                m.delimiters.as_str(),
                m.drop.as_str(),
                m.conditional.as_str()
            ),
            (" ,.<>", "#", "()")
        );
        assert!(m.noise.ends_with("zebra"));

        // fixed: the FIXED (not maintained) index, default options otherwise.
        let fixed = options("fixed.adi", "M_FIXED");
        assert_eq!(fixed.options, 0x60);
        assert_eq!(fixed.delimiters, default_delims);
    }

    /// Every `.adt` fixture with indexes in its JSON dump, with the dump.
    fn indexed_tables() -> Vec<(PathBuf, Json)> {
        let mut tables = Vec::new();
        for dir in std::fs::read_dir(fixtures()).unwrap() {
            for entry in std::fs::read_dir(dir.unwrap().path()).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_some_and(|e| e == "adt") {
                    let json = std::fs::read_to_string(path.with_extension("json")).unwrap();
                    let dump: Json = serde_json::from_str(&json).unwrap();
                    if dump["indexes"].as_array().is_some_and(|i| !i.is_empty()) {
                        tables.push((path, dump));
                    }
                }
            }
        }
        tables.sort_by(|a, b| a.0.cmp(&b.0));
        tables
    }

    /// `expression` with `F<n>` replaced by the names of `fields`.
    fn with_names(expression: &str, fields: &[Json]) -> String {
        let mut text = String::new();
        let mut chars = expression.chars().peekable();
        let mut previous = ' ';
        while let Some(c) = chars.next() {
            let starts_word = !previous.is_ascii_alphanumeric() && previous != '_';
            if c == 'F' && starts_word && chars.peek().is_some_and(char::is_ascii_digit) {
                let mut n = String::new();
                while let Some(&d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                    n.push(d);
                    chars.next();
                }
                let n: usize = n.parse().unwrap();
                text += fields[n - 1]["name"].as_str().unwrap();
                previous = '0';
            } else {
                text.push(c);
                previous = c;
            }
        }
        text
    }

    #[test]
    fn tags_match_the_engine() {
        let mut checked = 0;
        for (path, dump) in indexed_tables() {
            let fields = dump["fields"].as_array().unwrap();
            let dir = path.parent().unwrap();
            let expected = dump["indexes"].as_array().unwrap();
            let mut files: Vec<&str> = Vec::new();
            for order in expected {
                let file = order["file"].as_str().unwrap();
                if !files.contains(&file) {
                    files.push(file);
                }
            }
            for file in files {
                let index = match IndexFile::open(&dir.join(file)) {
                    Ok(index) => index,
                    Err(Error::Encrypted) => continue,
                    Err(error) => panic!("{file}: {error:?}"),
                };
                let orders: Vec<&Json> = expected
                    .iter()
                    .filter(|o| o["file"].as_str() == Some(file))
                    .collect();
                let names: Vec<&str> = index.tags.iter().map(|t| t.name.as_str()).collect();
                let want: Vec<&str> = orders.iter().map(|o| o["name"].as_str().unwrap()).collect();
                assert_eq!(names, want, "{file}: orders in creation order");
                for (tag, order) in index.tags.iter().zip(orders) {
                    let at = format!("{file} {}", tag.name);
                    let expression = with_names(&tag.expression, fields);
                    assert!(
                        expression.eq_ignore_ascii_case(order["expression"].as_str().unwrap()),
                        "{at}: {expression}"
                    );
                    let condition = with_names(&tag.condition, fields);
                    assert!(
                        condition.eq_ignore_ascii_case(order["condition"].as_str().unwrap()),
                        "{at}: {condition}"
                    );
                    assert_eq!(u64::from(tag.key_length), order["key_length"], "{at}");
                    assert_eq!(tag.unique(), order["unique"], "{at}");
                    assert_eq!(tag.custom(), order["custom"], "{at}");
                    assert_eq!(tag.descending, order["descending"], "{at}");
                    assert_eq!(tag.full_text(), order["fts"], "{at}");
                    // Unicode orders report a locale (`:en_US`), whose place in
                    // the file is not known.
                    let collation = order["collation"].as_str().unwrap();
                    if !collation.starts_with(':') {
                        assert_eq!(tag.collation, collation, "{at}");
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked >= 50, "checked only {checked} orders");
    }

    #[test]
    fn opens_an_encrypted_adi_with_the_table_password() {
        let path = fixtures().join("dictionary_encrypted/rc4_secret.adi");
        // Refused without the password, opened with the table's.
        assert!(matches!(IndexFile::open(&path), Err(Error::Encrypted)));
        let cipher = RecordCipher::Rc4(b"tblpass".to_vec());
        let mut index = IndexFile::open_encrypted(&path, &cipher).unwrap();
        let tag = index
            .tags
            .iter()
            .find(|t| t.name == "BY_NAME")
            .unwrap()
            .clone();
        let keys: Vec<(u32, String)> = index
            .keys(&tag)
            .unwrap()
            .iter()
            .map(|k| {
                (
                    k.record,
                    k.bytes.iter().map(|b| format!("{b:02x}")).collect(),
                )
            })
            .collect();
        // The names in key order (deciphered): "also hidden" then "hidden".
        assert_eq!(
            keys,
            [
                (2, "616c736f2068696464656e202020202020202020".to_owned()),
                (1, "68696464656e2020202020202020202020202020".to_owned()),
            ]
        );
    }

    #[test]
    fn opens_an_aes_index() {
        let aes = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/aes/aes256");
        let table =
            crate::adt::open_with_password(&aes.join("big.adt"), false, Some(b"ddsecret")).unwrap();
        let cipher = table.encryption_cipher().unwrap();
        let mut index = IndexFile::open_encrypted(&aes.join("big.adi"), &cipher).unwrap();
        let names: Vec<&str> = index.tags.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            ["BIG_WORD", "BIG_N", "BIG_ID"],
            "tag directory deciphered"
        );
        let tag = index
            .tags
            .iter()
            .find(|t| t.name == "BIG_ID")
            .unwrap()
            .clone();
        let keys = index.keys(&tag).unwrap();
        assert_eq!(keys.len(), 400);
        assert_eq!(keys.first().map(|k| k.record), Some(1));
    }

    /// Enciphering an AES index tree page reproduces the engine's bytes: every
    /// tree page of `big.adi`, deciphered and re-enciphered with its own IV,
    /// comes back byte for byte (the leaf-displacement inverse included). The
    /// engine cannot be asked to re-make the file (it refuses AES under Wine), so
    /// this pins the writer against the engine's own index.
    #[test]
    fn aes_index_pages_re_encipher_as_the_engine() {
        let aes = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/aes/aes256");
        let table =
            crate::adt::open_with_password(&aes.join("big.adt"), false, Some(b"ddsecret")).unwrap();
        let (key, nonce) = match table.encryption_cipher().unwrap() {
            RecordCipher::Aes { key, nonce } => (key, nonce),
            _ => panic!("big.adt is not AES"),
        };
        let cipher = RecordCipher::Aes {
            key: key.clone(),
            nonce,
        };
        let index = IndexFile::open_encrypted(&aes.join("big.adi"), &cipher).unwrap();
        let page_size = index.page_size as usize;
        let tags: Vec<Tag> = index.tags.clone();
        drop(index);
        let raw = std::fs::read(aes.join("big.adi")).unwrap();
        let get = |p: u32| raw[p as usize * page_size..(p as usize + 1) * page_size].to_vec();

        // Walk every tag's tree, collecting its page numbers (interior children
        // are big-endian after each entry's key and record).
        let mut pages = std::collections::BTreeSet::new();
        for tag in &tags {
            let mut stack = vec![tag.root];
            while let Some(p) = stack.pop() {
                if !pages.insert(p) {
                    continue;
                }
                let mut page = get(p);
                aes_decipher_page(&key, &nonce, &mut page);
                if !is_leaf(&page) {
                    let count = usize::from(u16_at(&page, 2));
                    let kl = usize::from(tag.key_length);
                    for i in 0..count {
                        let at = NODE_HEADER_SIZE + i * (kl + 8) + kl + 4;
                        let child = u32::from_be_bytes(page[at..at + 4].try_into().unwrap());
                        if child != 0 && (child as usize + 1) * page_size <= raw.len() {
                            stack.push(child);
                        }
                    }
                }
            }
        }
        assert!(pages.len() >= 4, "only walked {} tree pages", pages.len());

        for p in pages {
            let original = get(p);
            let mut clear = original.clone();
            aes_decipher_page(&key, &nonce, &mut clear);
            let iv: [u8; 8] = original[page_size - 8..].try_into().unwrap();
            let mut built = clear;
            aes_encipher_page(&key, &nonce, &iv, &mut built);
            assert_eq!(built, original, "page {p}");
        }
    }

    /// Maintaining an AES index enciphers the pages it rewrites: a key inserted
    /// into `big.adi` (on a scratch copy) is found again after reopening, so the
    /// written pages deciphered correctly and the untouched ones were left
    /// readable. The engine cannot verify AES, so the round-trip is tflip's own.
    #[test]
    fn an_aes_index_maintains_enciphered() {
        let aes = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/aes/aes256");
        let table =
            crate::adt::open_with_password(&aes.join("big.adt"), false, Some(b"ddsecret")).unwrap();
        let cipher = table.encryption_cipher().unwrap();
        let dir = crate::testing::temp_root().join(format!("tflip-aesidx-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let adi = dir.join("big.adi");
        std::fs::copy(aes.join("big.adi"), &adi).unwrap();

        // A key of the right length for BIG_WORD, inserted for a new record.
        let (name, probe, record) = ("BIG_WORD", vec![0x7Au8; 10], 9999);
        {
            let mut index = IndexFile::open_encrypted_for_writing(&adi, &cipher).unwrap();
            index.set_aes_counter(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
                1 << 40,
            )));
            let tag = index.tags.iter().find(|t| t.name == name).unwrap().clone();
            assert_eq!(usize::from(tag.key_length), probe.len());
            index.insert_key(&tag, &probe, record).unwrap();
        }
        // Reopen: the inserted key reads back, and the original 400 keys remain.
        let mut index = IndexFile::open_encrypted(&adi, &cipher).unwrap();
        let tag = index.tags.iter().find(|t| t.name == name).unwrap().clone();
        assert!(
            index.has_key(&tag, &probe, Some(record)).unwrap(),
            "inserted key found"
        );
        assert_eq!(index.keys(&tag).unwrap().len(), 401, "original keys kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_match_the_engine() {
        let mut checked = 0;
        for (path, dump) in indexed_tables() {
            let dir = path.parent().unwrap();
            for order in dump["indexes"].as_array().unwrap() {
                let Some(expected) = order["keys"].as_array() else {
                    continue;
                };
                let file = order["file"].as_str().unwrap();
                let mut index = match IndexFile::open(&dir.join(file)) {
                    Ok(index) => index,
                    Err(Error::Encrypted) => continue,
                    Err(error) => panic!("{file}: {error:?}"),
                };
                let name = order["name"].as_str().unwrap();
                let tag = index.tags.iter().find(|t| t.name == name).unwrap().clone();
                let keys: Vec<(u32, String)> = index
                    .keys(&tag)
                    .unwrap()
                    .iter()
                    .map(|k| {
                        (
                            k.record,
                            k.bytes.iter().map(|b| format!("{b:02x}")).collect(),
                        )
                    })
                    .collect();
                let want: Vec<(u32, String)> = expected
                    .iter()
                    .map(|k| {
                        let record = k["recno"].as_u64().unwrap() as u32;
                        (record, k["key"].as_str().unwrap().to_lowercase())
                    })
                    .collect();
                assert_eq!(keys, want, "{file} {name}");
                checked += 1;
            }
        }
        assert!(checked >= 50, "checked only {checked} orders");
    }

    /// Every order of the fixtures that is not encrypted, opened, with its
    /// keys in index order as [`IndexFile::keys`] reads them.
    fn orders() -> Vec<(String, IndexFile, Tag, Vec<Key>)> {
        let mut orders = Vec::new();
        for (path, dump) in indexed_tables() {
            let dir = path.parent().unwrap();
            for order in dump["indexes"].as_array().unwrap() {
                let file = order["file"].as_str().unwrap();
                let mut index = match IndexFile::open(&dir.join(file)) {
                    Ok(index) => index,
                    Err(Error::Encrypted) => continue,
                    Err(error) => panic!("{file}: {error:?}"),
                };
                let name = order["name"].as_str().unwrap();
                let tag = index.tags.iter().find(|t| t.name == name).unwrap().clone();
                let keys = index.keys(&tag).unwrap();
                orders.push((
                    format!("{} {name}", dir.join(file).display()),
                    index,
                    tag,
                    keys,
                ));
            }
        }
        assert!(orders.len() >= 50, "only {} orders", orders.len());
        orders
    }

    /// The keys from the cursor's position on, moving with `step` until it
    /// runs off the keys.
    fn walk(
        cursor: &mut Cursor,
        index: &mut IndexFile,
        step: for<'a> fn(&'a mut Cursor, &mut IndexFile) -> Result<Option<&'a Key>, Error>,
    ) -> Vec<Key> {
        let mut keys = Vec::new();
        while let Some(key) = cursor.key() {
            keys.push(key.clone());
            step(cursor, index).unwrap();
        }
        keys
    }

    #[test]
    fn cursors_walk_like_the_keys() {
        for (at, mut index, tag, keys) in orders() {
            let mut cursor = Cursor::new(&tag);
            assert!(cursor.before_first() && cursor.key().is_none(), "{at}");
            assert_eq!(
                cursor.next(&mut index).unwrap(),
                keys.first(),
                "{at}: next from the start"
            );
            assert_eq!(walk(&mut cursor, &mut index, Cursor::next), keys, "{at}");
            assert!(cursor.after_last(), "{at}");
            assert_eq!(
                cursor.next(&mut index).unwrap(),
                None,
                "{at}: stays after the last"
            );
            assert_eq!(
                cursor.prev(&mut index).unwrap(),
                keys.last(),
                "{at}: back from the end"
            );
            let mut back = walk(&mut cursor, &mut index, Cursor::prev);
            back.reverse();
            assert_eq!(back, keys, "{at}");
            assert!(cursor.before_first(), "{at}");
            assert_eq!(
                cursor.prev(&mut index).unwrap(),
                None,
                "{at}: stays before the first"
            );

            assert_eq!(cursor.last(&mut index).unwrap(), keys.last(), "{at}");
            assert_eq!(cursor.first(&mut index).unwrap(), keys.first(), "{at}");
            if keys.is_empty() {
                assert!(cursor.after_last(), "{at}: first of no keys");
                assert_eq!(cursor.last(&mut index).unwrap(), None, "{at}");
                assert!(cursor.before_first(), "{at}: last of no keys");
            }
            assert_eq!(
                cursor.key_count(&mut index).unwrap(),
                keys.len() as u32,
                "{at}"
            );
        }
    }

    /// Every `n`th of `len` keys, for `n` that keeps it to about 500.
    fn sample(len: usize) -> usize {
        len / 500 + 1
    }

    #[test]
    fn cursors_seek_keys() {
        for (at, mut index, tag, keys) in orders() {
            let mut cursor = Cursor::new(&tag);
            for (i, key) in keys.iter().enumerate().step_by(sample(keys.len())) {
                let found = cursor
                    .seek(&mut index, &key.bytes, Some(key.record))
                    .unwrap();
                assert_eq!(found, Some(key), "{at}: key {i}");
                // Counting reads the leaves before; every 10th is enough.
                if i % 10 == 0 {
                    assert_eq!(
                        cursor.key_number(&mut index).unwrap(),
                        Some(i as u32 + 1),
                        "{at}"
                    );
                }
            }
            if let Some(last) = keys.last() {
                cursor
                    .seek(&mut index, &last.bytes, Some(last.record))
                    .unwrap();
                let number = cursor.key_number(&mut index).unwrap();
                assert_eq!(number, Some(keys.len() as u32), "{at}: the last key");
            }
            cursor.first(&mut index).unwrap();
            cursor.prev(&mut index).unwrap();
            assert_eq!(
                cursor.key_number(&mut index).unwrap(),
                None,
                "{at}: before the first"
            );
        }
    }

    /// Seeks for keys that are not in the order (between keys, shorter
    /// ones, before and after all) land where a search through all the keys
    /// says.
    #[test]
    fn cursors_seek_between_keys() {
        for (at, mut index, tag, keys) in orders() {
            let length = usize::from(tag.key_length);
            let mut targets = vec![vec![0x00; length], vec![0xFF; length], vec![]];
            for key in keys.iter().step_by(sample(keys.len()) * 4) {
                let bytes = &key.bytes;
                targets.push(bytes.clone());
                targets.push(bytes[..bytes.len() / 2].to_vec());
                if let Some(last) = bytes.iter().rposition(|&b| b < 0xFF) {
                    let mut above = bytes[..=last].to_vec();
                    above[last] += 1;
                    targets.push(above);
                }
                if let Some(last) = bytes.iter().rposition(|&b| b > 0x00) {
                    let mut below = bytes[..=last].to_vec();
                    below[last] -= 1;
                    targets.push(below);
                }
            }
            let mut cursor = Cursor::new(&tag);
            for target in targets {
                // In index order: the first key not below the target
                // (ascending) or not above it (descending).
                let place = keys.partition_point(|key| match tag.descending {
                    false => compare(key, &target, None) == Ordering::Less,
                    true => compare(key, &target, None) == Ordering::Greater,
                });
                let found = cursor.seek(&mut index, &target, None).unwrap().cloned();
                assert_eq!(found.as_ref(), keys.get(place), "{at}: {target:02x?}");
                assert_eq!(
                    cursor.after_last(),
                    place == keys.len(),
                    "{at}: {target:02x?}"
                );
            }
        }
    }
}
