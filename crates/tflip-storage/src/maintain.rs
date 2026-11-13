//! Keeping ADI, CDX and IDX orders current as records change: keys inserted
//! and deleted in an order's B+ tree as the original engine does it, so that
//! the pages it leaves are the engine's byte for byte
//! (`fixtures/write/indexes`, `fixtures/write/cdx_indexes`).
//!
//! The two formats share the tree. A CDX file's nodes are 512 bytes and
//! named by their byte offsets (an ADI file's by page numbers), its keys
//! compare by its static collation's weights, an order with a `FOR` condition
//! has no trailing compression, and a free list may end in 0xFFFF_FFFF.
//!
//! - **Leaves** are written as compact leaves (see [`crate::adi`]): each
//!   key's trailing pad bytes are counted first; its duplicate count is the
//!   prefix it shares with the key before, taking nothing from either key's
//!   trailing bytes; the key bytes are packed from the page's end, and the
//!   free space between them and the entries is zero.
//! - **Record numbers** too wide for a leaf's layout widen it: the bits of
//!   the record number (at least 16), plus what makes the duplicate and
//!   trailing counts end on a byte, at least 12, rounded up to whole bytes.
//! - **Insert**: the descent takes the first child whose last key (and
//!   record) is not below the new one, the last if none is. A full node
//!   splits: the last leaf of the order, when the key goes at its end, keeps
//!   its keys and the key starts a new leaf, and so does its parent, if it
//!   is the last of its level and splits too, and so on up; any other node
//!   (also the last interior node above a leaf that halved) keeps its lower
//!   half and gives the upper ⌊n/2⌋ of its n old entries to a new page, the
//!   new key joining the half it belongs in. The parent's entry for the node
//!   then points to the new page, and one for the node goes before it. A full
//!   root keeps its page: both halves go to new pages and it becomes an
//!   interior node of two entries. A split leaf keeps its old layout, but for
//!   the half that takes a key that widens it.
//! - **Delete**: the key leaves its leaf and the parents are brought up to
//!   date; a node that empties is unlinked from its siblings, its page goes on
//!   the order's free list (tag header 0x04: zeroed, the next free page at
//!   offset 4), and its entry leaves its parent. Nothing is merged. A tree
//!   that empties leaves an empty root leaf without a layout.
//! - New pages come from the order's free list, else from the file's end.
//! - **Binary orders** (`DELETED()`) keep a bit per record slot, set for a
//!   deleted record: a slot added is a bit added to the last leaf (a full
//!   leaf starts a new one; the first time the root leaf moves to a new page
//!   and the root becomes an interior node of (last record number the leaf
//!   can hold, leaf) entries).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use crate::table::RecordCipher;

use crate::adi::{IndexFile, TreeRef, decode_leaf};
use crate::cdx::CdxFile;
use crate::order::{Error, Key, Tag};

const NONE: u32 = 0xFFFF_FFFF;
const ROOT: u16 = 0x01;
const LEAF: u16 = 0x02;
const NODE_HEADER: usize = 12;
const LEAF_HEADER: usize = 24;
const BITMAP_OFFSET: usize = 16;

/// A compact leaf's layout: the bits of the record number, the duplicate
/// count and the trailing count, and the bytes an entry takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    pub r: u32,
    pub d: u32,
    pub t: u32,
    pub s: usize,
}

/// The bits `n` takes.
fn bits(n: u32) -> u32 {
    32 - n.leading_zeros()
}

impl Layout {
    /// The layout of a leaf that takes record number `record` too wide for
    /// it.
    /// The layout of a new order's empty root leaf: record numbers of the
    /// fewest bits, at least 12, that make an entry whole bytes.
    pub(crate) fn initial(key_length: usize) -> Layout {
        let d = bits(key_length as u32);
        let t = d;
        let mut r = 12;
        while !(r + d + t).is_multiple_of(8) {
            r += 1;
        }
        Layout {
            r,
            d,
            t,
            s: ((r + d + t) / 8) as usize,
        }
    }

    pub(crate) fn widened(key_length: usize, record: u32) -> Layout {
        let d = bits(key_length as u32);
        let t = d;
        let mut r = ((8 - (2 * t) % 8) % 8 + bits(record.max(0xFFFF))).max(12);
        while !(r + d + t).is_multiple_of(8) {
            r += 1;
        }
        Layout {
            r,
            d,
            t,
            s: ((r + d + t) / 8) as usize,
        }
    }

    /// The layout with room for a word's count after the entry's bits, as a
    /// full-text order that keeps scores has it (`fixtures/write/fts_build`).
    pub(crate) fn scored(self, score: bool) -> Layout {
        Layout {
            s: self.s + if score { 2 } else { 0 },
            ..self
        }
    }

    /// Whether an entry has a word's count after its bits.
    fn has_score(self) -> bool {
        self.s * 8 >= (self.r + self.d + self.t) as usize + 16
    }

    pub(crate) fn mask(bits: u32) -> u64 {
        if bits >= 64 {
            u64::MAX
        } else {
            (1 << bits) - 1
        }
    }
}

/// A node of a tree, decoded.
#[derive(Debug, Clone)]
struct Node {
    leaf: bool,
    root: bool,
    left: u32,
    right: u32,
    /// A leaf's layout.
    layout: Layout,
    /// A leaf's keys, or an interior node's last key of each child.
    keys: Vec<Key>,
    /// An interior node's children.
    children: Vec<u32>,
}

impl Node {
    /// The last key and record, which the parent's entry holds.
    fn last(&self) -> Key {
        self.keys.last().cloned().expect("a node with keys")
    }
}

/// The trailing count of `key` in a leaf of `layout` with pad byte `pad`
/// (none: no trailing compression).
fn trailing(key: &[u8], layout: Layout, pad: Option<u8>) -> usize {
    let tmask = Layout::mask(layout.t) as usize;
    match pad {
        Some(pad) => key
            .iter()
            .rev()
            .take_while(|&&b| b == pad)
            .count()
            .min(tmask),
        None => 0,
    }
}

/// The duplicate count of `key`, whose trailing count is `trail`, after
/// `previous` and its trailing count in a leaf of `layout`.
fn duplicates(previous: Option<(&[u8], usize)>, key: &[u8], trail: usize, layout: Layout) -> usize {
    let dmask = Layout::mask(layout.d) as usize;
    let mut dup = 0;
    if let Some((previous, previous_trail)) = previous {
        let limit = (key.len() - trail)
            .min(previous.len() - previous_trail)
            .min(dmask);
        while dup < limit && previous[dup] == key[dup] {
            dup += 1;
        }
    }
    dup
}

/// A compact leaf's place in its tree: whether it is the root, and its
/// siblings.
pub(crate) struct Compact {
    pub root: bool,
    pub left: u32,
    pub right: u32,
}

impl Compact {
    /// A root leaf with no siblings.
    pub(crate) fn empty_root() -> Compact {
        Compact {
            root: true,
            left: NONE,
            right: NONE,
        }
    }

    /// The leaf of `keys` in a page of `page_size` bytes, laid out as
    /// `layout` says, trailing `pad` bytes compressed (none: no trailing
    /// compression); `None` if they do not fit.
    pub(crate) fn bytes(
        &self,
        keys: &[Key],
        layout: Layout,
        pad: Option<u8>,
        page_size: usize,
    ) -> Option<Vec<u8>> {
        self.bytes_keeping(keys, layout, pad, page_size, &|_| None, &|_| 0)
    }

    /// [`Compact::bytes`], with each key's word count from `score` (a layout
    /// with room for them, see [`Layout::scored`]).
    pub(crate) fn bytes_scored(
        &self,
        keys: &[Key],
        layout: Layout,
        pad: Option<u8>,
        page_size: usize,
        score: &dyn Fn(&Key) -> u16,
    ) -> Option<Vec<u8>> {
        self.bytes_keeping(keys, layout, pad, page_size, &|_| None, score)
    }

    /// [`Compact::bytes`], but the keys `kept` gives a trailing count for
    /// keep it.
    fn bytes_keeping(
        &self,
        keys: &[Key],
        layout: Layout,
        pad: Option<u8>,
        page_size: usize,
        kept: &dyn Fn(&Key) -> Option<usize>,
        score: &dyn Fn(&Key) -> u16,
    ) -> Option<Vec<u8>> {
        let (mut entries, mut stored) = (Vec::new(), Vec::new());
        let mut previous: Option<(&[u8], usize)> = None;
        for key in keys {
            let trail = kept(key).unwrap_or_else(|| trailing(&key.bytes, layout, pad));
            let dup = duplicates(previous, &key.bytes, trail, layout);
            let mut value = u64::from(key.record)
                | (dup as u64) << layout.r
                | (trail as u64) << (layout.r + layout.d);
            if layout.has_score() {
                value |= u64::from(score(key)) << (layout.r + layout.d + layout.t);
            }
            entries.extend_from_slice(&value.to_le_bytes()[..layout.s]);
            let part = &key.bytes[dup..key.bytes.len() - trail];
            stored.splice(0..0, part.iter().copied());
            previous = Some((&key.bytes, trail));
        }
        let free = page_size.checked_sub(LEAF_HEADER + entries.len() + stored.len())?;
        let mut page = Vec::with_capacity(page_size);
        let attributes = LEAF | if self.root { ROOT } else { 0 };
        page.extend_from_slice(&attributes.to_le_bytes());
        page.extend_from_slice(&(keys.len() as u16).to_le_bytes());
        page.extend_from_slice(&self.left.to_le_bytes());
        page.extend_from_slice(&self.right.to_le_bytes());
        page.extend_from_slice(&(free as u16).to_le_bytes());
        let record_mask = if layout.r >= 32 {
            u32::MAX
        } else {
            (1 << layout.r) - 1
        };
        page.extend_from_slice(&record_mask.to_le_bytes());
        page.push(Layout::mask(layout.d) as u8);
        // The trailing count's mask is its bits above the eighth in a leaf of
        // keys over 255 bytes (`fixtures/write/limits`).
        let high = if layout.t > 8 { 8 } else { 0 };
        page.push((Layout::mask(layout.t) >> high) as u8);
        page.extend_from_slice(&[
            layout.r as u8,
            layout.d as u8,
            layout.t as u8,
            layout.s as u8,
        ]);
        page.extend_from_slice(&entries);
        page.resize(page.len() + free, 0);
        page.extend_from_slice(&stored);
        Some(page)
    }
}

/// How an index format lays out its trees: the size of a node, and the
/// bytes a node number counts (the page size, or 1 for byte offsets).
#[derive(Debug, Clone, Copy)]
struct Geometry {
    page_size: usize,
    unit: u64,
}

/// The order a tree belongs to, open for maintenance.
struct Tree<'f> {
    file: &'f mut File,
    geometry: Geometry,
    page_size: usize,
    /// Where the tag header starts in the file.
    header: u64,
    root: u32,
    key_length: usize,
    /// The byte trailing compression removes, if the order has it.
    pad: Option<u8>,
    /// The weights keys compare by (a static collation's), if not their
    /// bytes.
    weights: Option<[u8; 256]>,
    /// For an AES-encrypted index (ADI only): the key and nonce its tree pages
    /// are enciphered with, and the shared IV counter write draws from. `None`
    /// for an unencrypted index or the RC4 family (whose index is left clear).
    aes: Option<(Vec<u8>, [u8; 4])>,
    aes_iv: Option<Arc<AtomicU64>>,
    /// For an RC4-encrypted index (a dictionary table's, ADI only): the key
    /// each tree page is one stream under, at its page number; a page nothing
    /// was written to is clear.
    rc4: Option<Vec<u8>>,
    /// The trailing counts of the keys read from leaves that store another
    /// one than `pad` gives them, which they keep when their leaves are
    /// written again, as in the original engine: a FoxPro order on a
    /// timestamp built with zeros left out and kept current with spaces
    /// (`fixtures/write/cdx_timestamps`).
    kept: HashMap<(u32, Vec<u8>), usize>,
    /// Whether the order is a full-text one that keeps scores, whose leaf
    /// entries count each word's occurrences.
    scored: bool,
    /// The word counts of the keys of the leaves read, and of keys to
    /// insert (see [`IndexFile::insert_scored`]).
    scores: HashMap<(u32, Vec<u8>), u16>,
}

impl Tree<'_> {
    fn page(&mut self, page: u32) -> Result<Vec<u8>, Error> {
        let mut bytes = vec![0; self.page_size];
        self.file
            .seek(SeekFrom::Start(u64::from(page) * self.geometry.unit))?;
        self.file.read_exact(&mut bytes)?;
        if let Some((key, nonce)) = &self.aes {
            crate::adi::aes_decipher_page(key, nonce, &mut bytes);
        }
        if let Some(key) = &self.rc4
            && bytes.iter().any(|&b| b != 0)
        {
            crate::crypto::rc4_crypt(key, page, &mut bytes);
        }
        Ok(bytes)
    }

    fn put(&mut self, page: u32, bytes: &[u8]) -> Result<(), Error> {
        self.file
            .seek(SeekFrom::Start(u64::from(page) * self.geometry.unit))?;
        // An AES index's tree pages are enciphered, each with a fresh IV drawn
        // from the shared counter so none collides with a record's or another
        // page's (see [`crate::adi::aes_encipher_page`]).
        if let (Some((key, nonce)), Some(counter)) = (&self.aes, &self.aes_iv) {
            let n = u64::from((self.page_size as u32).div_ceil(16).max(1));
            let iv = counter
                .fetch_add(n, std::sync::atomic::Ordering::Relaxed)
                .to_le_bytes();
            let mut enciphered = bytes.to_vec();
            crate::adi::aes_encipher_page(key, nonce, &iv, &mut enciphered);
            return Ok(self.file.write_all(&enciphered)?);
        }
        if let Some(key) = &self.rc4 {
            let mut enciphered = bytes.to_vec();
            crate::crypto::rc4_crypt(key, page, &mut enciphered);
            return Ok(self.file.write_all(&enciphered)?);
        }
        Ok(self.file.write_all(bytes)?)
    }

    fn header_u32(&mut self, offset: u64) -> Result<u32, Error> {
        let mut bytes = [0; 4];
        self.file.seek(SeekFrom::Start(self.header + offset))?;
        self.file.read_exact(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn set_header_u32(&mut self, offset: u64, value: u32) -> Result<(), Error> {
        self.file.seek(SeekFrom::Start(self.header + offset))?;
        Ok(self.file.write_all(&value.to_le_bytes())?)
    }

    /// A page for a new node: the head of the order's free list, else one
    /// past the file's end.
    fn new_page(&mut self) -> Result<u32, Error> {
        let free = self.header_u32(4)?;
        if free != 0 && free != NONE {
            let next = u32::from_le_bytes(self.page(free)?[4..8].try_into().unwrap());
            self.set_header_u32(4, next)?;
            return Ok(free);
        }
        let length = self.file.metadata()?.len();
        Ok((length / self.geometry.unit) as u32)
    }

    /// Puts page `page` on the order's free list: zeros, but for the page
    /// that headed the list before at offset 4.
    fn free_page(&mut self, page: u32) -> Result<(), Error> {
        let mut bytes = vec![0; self.page_size];
        let next = match self.header_u32(4)? {
            NONE => 0,
            next => next,
        };
        bytes[4..8].copy_from_slice(&next.to_le_bytes());
        self.put(page, &bytes)?;
        self.set_header_u32(4, page)
    }

    fn tree_ref(&self) -> TreeRef {
        TreeRef {
            root: self.root,
            key_length: self.key_length as u16,
            pad: self.pad.unwrap_or(0),
            binary: false,
        }
    }

    /// Whether `a` sorts before `b`: by key (weighed), then record number.
    fn below(&self, a: &Key, b: &Key) -> bool {
        self.order(&a.bytes, &b.bytes).then(a.record.cmp(&b.record)) == Ordering::Less
    }

    /// How two keys compare, by the order's weights.
    fn order(&self, a: &[u8], b: &[u8]) -> Ordering {
        crate::collation::weighed_cmp(a, b, self.weights.as_ref())
    }

    fn read(&mut self, page: u32) -> Result<Node, Error> {
        let bytes = self.page(page)?;
        let attributes = u16::from_le_bytes([bytes[0], bytes[1]]);
        let count = usize::from(u16::from_le_bytes([bytes[2], bytes[3]]));
        let left = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let right = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let root = attributes & ROOT != 0;
        if attributes & LEAF != 0 {
            let layout = Layout {
                r: u32::from(bytes[20]),
                d: u32::from(bytes[21]),
                t: u32::from(bytes[22]),
                s: usize::from(bytes[23]),
            };
            let mut keys = Vec::new();
            decode_leaf(&bytes, &self.tree_ref(), &mut keys)?;
            for (i, key) in keys.iter().enumerate() {
                let at = LEAF_HEADER + i * layout.s;
                let mut entry = [0; 8];
                entry[..layout.s].copy_from_slice(&bytes[at..at + layout.s]);
                let value = u64::from_le_bytes(entry) >> (layout.r + layout.d);
                let trail = (value & Layout::mask(layout.t)) as usize;
                if trail != trailing(&key.bytes, layout, self.pad) {
                    self.kept.insert((key.record, key.bytes.clone()), trail);
                }
                if layout.has_score() {
                    let score =
                        (u64::from_le_bytes(entry) >> (layout.r + layout.d + layout.t)) as u16;
                    self.scores.insert((key.record, key.bytes.clone()), score);
                }
            }
            return Ok(Node {
                leaf: true,
                root,
                left,
                right,
                layout,
                keys,
                children: Vec::new(),
            });
        }
        let entry = self.key_length + 8;
        let (mut keys, mut children) = (Vec::new(), Vec::new());
        for i in 0..count {
            let at = NODE_HEADER + i * entry;
            let k = self.key_length;
            let record = u32::from_be_bytes(bytes[at + k..at + k + 4].try_into().unwrap());
            keys.push(Key {
                bytes: bytes[at..at + k].to_vec(),
                record,
            });
            children.push(u32::from_be_bytes(
                bytes[at + k + 4..at + k + 8].try_into().unwrap(),
            ));
        }
        let layout = Layout {
            r: 0,
            d: 0,
            t: 0,
            s: 0,
        };
        Ok(Node {
            leaf: false,
            root,
            left,
            right,
            layout,
            keys,
            children,
        })
    }

    /// A leaf's page, or `None` if its keys do not fit.
    fn leaf_bytes(&self, node: &Node) -> Option<Vec<u8>> {
        let leaf = Compact {
            root: node.root,
            left: node.left,
            right: node.right,
        };
        let kept = |key: &Key| self.kept.get(&(key.record, key.bytes.clone())).copied();
        let score = |key: &Key| {
            self.scores
                .get(&(key.record, key.bytes.clone()))
                .copied()
                .unwrap_or(0)
        };
        leaf.bytes_keeping(
            &node.keys,
            node.layout,
            self.pad,
            self.page_size,
            &kept,
            &score,
        )
    }

    fn fits(&self, node: &Node) -> bool {
        match node.leaf {
            true => self.leaf_bytes(node).is_some(),
            false => NODE_HEADER + node.keys.len() * (self.key_length + 8) <= self.page_size,
        }
    }

    fn write(&mut self, page: u32, node: &Node) -> Result<(), Error> {
        if node.leaf {
            let bytes = self
                .leaf_bytes(node)
                .ok_or(Error::Corrupt("leaf overflows"))?;
            return self.put(page, &bytes);
        }
        let mut bytes = vec![0; self.page_size];
        let attributes: u16 = if node.root { ROOT } else { 0 };
        bytes[0..2].copy_from_slice(&attributes.to_le_bytes());
        bytes[2..4].copy_from_slice(&(node.keys.len() as u16).to_le_bytes());
        bytes[4..8].copy_from_slice(&node.left.to_le_bytes());
        bytes[8..12].copy_from_slice(&node.right.to_le_bytes());
        let k = self.key_length;
        for (i, (key, child)) in node.keys.iter().zip(&node.children).enumerate() {
            let at = NODE_HEADER + i * (k + 8);
            bytes[at..at + k].copy_from_slice(&key.bytes);
            bytes[at + k..at + k + 4].copy_from_slice(&key.record.to_be_bytes());
            bytes[at + k + 4..at + k + 8].copy_from_slice(&child.to_be_bytes());
        }
        self.put(page, &bytes)
    }

    /// The interior nodes from the root down to the leaf for `key` (each with
    /// the index of the entry taken), the leaf's page, and the leaf.
    #[allow(clippy::type_complexity)]
    fn path(&mut self, key: &Key) -> Result<(Vec<(u32, Node, usize)>, u32, Node), Error> {
        let mut page = self.root;
        let mut path = Vec::new();
        for _ in 0..32 {
            let node = self.read(page)?;
            if node.leaf {
                return Ok((path, page, node));
            }
            if node.keys.is_empty() {
                return Err(Error::Corrupt("empty interior node"));
            }
            let i = node
                .keys
                .iter()
                .position(|k| !self.below(k, key))
                .unwrap_or(node.keys.len() - 1);
            let child = node.children[i];
            path.push((page, node, i));
            page = child;
        }
        Err(Error::Corrupt("tree too deep"))
    }

    fn insert(&mut self, key: Key) -> Result<(), Error> {
        let (path, page, mut leaf) = self.path(&key)?;
        if leaf.keys.contains(&key) {
            return Err(Error::Corrupt("key already in the order"));
        }
        let position = leaf
            .keys
            .iter()
            .position(|k| self.below(&key, k))
            .unwrap_or(leaf.keys.len());
        let old_layout = leaf.layout;
        let wide = (bits(key.record) > old_layout.r)
            .then(|| Layout::widened(self.key_length, key.record).scored(self.scored));
        leaf.keys.insert(position, key.clone());
        if let Some(wide) = wide {
            leaf.layout = wide;
        }
        if self.fits(&leaf) {
            self.write(page, &leaf)?;
            return self.fix_up(&path, leaf.last());
        }
        leaf.layout = old_layout;
        let keys = leaf.keys.clone();
        let appended = leaf.right == NONE && position == keys.len() - 1;
        let (lo, hi) = match appended {
            true => (
                keys[..keys.len() - 1].to_vec(),
                keys[keys.len() - 1..].to_vec(),
            ),
            false => halve(&keys, position),
        };
        let wide = wide.map(|layout| (usize::from(!lo.contains(&key)), layout));
        let children = (Vec::new(), Vec::new());
        let (lo, hi) = ((lo, children.0), (hi, children.1));
        self.split(path, page, leaf, lo, hi, wide, appended)
    }

    /// Splits `node` at `page` into the entries `lo` and `hi` (with their
    /// children, in an interior node), and goes on up the path. `wide`: the
    /// half (0 lower, 1 upper) that takes a widened layout, and the layout.
    /// `appended`: whether the node split as the last of its level does when
    /// the new entry goes at its end, keeping its entries; only then does
    /// its parent, if it is the last of its level and splits too (as in the
    /// original engine: `fixtures/write/cdx_timestamps`, `cdx_indexes`).
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    fn split(
        &mut self,
        mut path: Vec<(u32, Node, usize)>,
        page: u32,
        node: Node,
        lo: (Vec<Key>, Vec<u32>),
        hi: (Vec<Key>, Vec<u32>),
        wide: Option<(usize, Layout)>,
        appended: bool,
    ) -> Result<(), Error> {
        let make = |(keys, children): (Vec<Key>, Vec<u32>), half: usize| {
            let mut half_node = Node {
                keys,
                children,
                ..node.clone()
            };
            if let Some((_, layout)) = wide.filter(|(h, _)| *h == half) {
                half_node.layout = layout;
            }
            half_node
        };
        if node.root {
            let a = self.new_page()?;
            // Taken before the next one is.
            let zeros = vec![0; self.page_size];
            self.put(a, &zeros)?;
            let b = self.new_page()?;
            let left = Node {
                root: false,
                left: NONE,
                right: b,
                ..make(lo, 0)
            };
            let right = Node {
                root: false,
                left: a,
                right: NONE,
                ..make(hi, 1)
            };
            self.write(a, &left)?;
            self.write(b, &right)?;
            let top = Node {
                leaf: false,
                root: true,
                left: NONE,
                right: NONE,
                layout: Layout {
                    r: 0,
                    d: 0,
                    t: 0,
                    s: 0,
                },
                keys: vec![left.last(), right.last()],
                children: vec![a, b],
            };
            return self.write(page, &top);
        }
        let m = self.new_page()?;
        let left = Node {
            right: m,
            ..make(lo, 0)
        };
        let right = Node {
            left: page,
            right: node.right,
            ..make(hi, 1)
        };
        if node.right != NONE {
            let mut sibling = self.read(node.right)?;
            sibling.left = m;
            self.write(node.right, &sibling)?;
        }
        self.write(page, &left)?;
        self.write(m, &right)?;
        let (parent_page, mut parent, i) =
            path.pop().ok_or(Error::Corrupt("split without parent"))?;
        // The parent's entry for the node now points to the new upper page,
        // and an entry for the lower half goes before it.
        parent.keys[i] = right.last();
        parent.children[i] = m;
        parent.keys.insert(i, left.last());
        parent.children.insert(i, page);
        if self.fits(&parent) {
            self.write(parent_page, &parent)?;
            return self.fix_up(&path, parent.last());
        }
        let n = parent.keys.len();
        let appended = appended && parent.right == NONE && i + 1 == n - 1;
        let (plo, phi) = match appended {
            true => (
                (
                    parent.keys[..n - 1].to_vec(),
                    parent.children[..n - 1].to_vec(),
                ),
                (
                    parent.keys[n - 1..].to_vec(),
                    parent.children[n - 1..].to_vec(),
                ),
            ),
            false => {
                let (klo, khi) = halve(&parent.keys, i);
                let (clo, chi) = halve(&parent.children, i);
                ((klo, clo), (khi, chi))
            }
        };
        self.split(path, parent_page, parent, plo, phi, None, appended)
    }

    /// Brings the parents' entries up to `last`, the last key of the node
    /// below them, as far as it changes them.
    fn fix_up(&mut self, path: &[(u32, Node, usize)], mut last: Key) -> Result<(), Error> {
        for (page, parent, i) in path.iter().rev() {
            let mut parent = parent.clone();
            if parent.keys[*i] == last {
                return Ok(());
            }
            parent.keys[*i] = last;
            self.write(*page, &parent)?;
            if *i != parent.keys.len() - 1 {
                return Ok(());
            }
            last = parent.last();
        }
        Ok(())
    }

    fn delete(&mut self, key: &Key) -> Result<(), Error> {
        let (path, page, mut leaf) = self.path(key)?;
        let at = leaf
            .keys
            .iter()
            .position(|k| k == key)
            .ok_or(Error::Corrupt("key not in the order"))?;
        leaf.keys.remove(at);
        if !leaf.keys.is_empty() || leaf.root {
            self.write(page, &leaf)?;
            if !leaf.keys.is_empty() {
                return self.fix_up(&path, leaf.last());
            }
            return Ok(());
        }
        self.unlink(path, page, leaf)
    }

    /// Removes the empty node at `page`: its siblings linked to each other,
    /// its page freed, its parent's entry dropped.
    fn unlink(
        &mut self,
        mut path: Vec<(u32, Node, usize)>,
        page: u32,
        node: Node,
    ) -> Result<(), Error> {
        if node.left != NONE {
            let mut sibling = self.read(node.left)?;
            sibling.right = node.right;
            self.write(node.left, &sibling)?;
        }
        if node.right != NONE {
            let mut sibling = self.read(node.right)?;
            sibling.left = node.left;
            self.write(node.right, &sibling)?;
        }
        self.free_page(page)?;
        let (parent_page, mut parent, i) =
            path.pop().ok_or(Error::Corrupt("node without parent"))?;
        parent.keys.remove(i);
        parent.children.remove(i);
        if parent.keys.is_empty() && parent.root {
            // An emptied tree: an empty root leaf without a layout.
            let mut bytes = vec![0; self.page_size];
            bytes[0..2].copy_from_slice(&(ROOT | LEAF).to_le_bytes());
            bytes[4..12].fill(0xFF);
            return self.put(parent_page, &bytes);
        }
        if !parent.keys.is_empty() || parent.root {
            self.write(parent_page, &parent)?;
            if !parent.keys.is_empty() {
                return self.fix_up(&path, parent.last());
            }
            return Ok(());
        }
        self.unlink(path, parent_page, parent)
    }
}

impl Tree<'_> {
    /// Whether the order has `key` of record `record`, or with no record of
    /// any record: looked for from where it would go, along the leaves until
    /// a key sorts after it.
    fn holders(&mut self, key: &[u8]) -> Result<Vec<u32>, Error> {
        let sought = Key {
            bytes: key.to_vec(),
            record: 0,
        };
        let (_, _, mut leaf) = self.path(&sought)?;
        let mut found = Vec::new();
        loop {
            for k in &leaf.keys {
                if k.bytes == key {
                    found.push(k.record);
                } else if self.order(&k.bytes, key) == Ordering::Greater {
                    return Ok(found);
                }
            }
            if leaf.right == NONE {
                return Ok(found);
            }
            leaf = self.read(leaf.right)?;
        }
    }

    fn has(&mut self, key: &[u8], record: Option<u32>) -> Result<bool, Error> {
        let sought = Key {
            bytes: key.to_vec(),
            record: record.unwrap_or(0),
        };
        let (_, _, mut leaf) = self.path(&sought)?;
        loop {
            for k in &leaf.keys {
                match record {
                    Some(_) if *k == sought => return Ok(true),
                    None if k.bytes == key => return Ok(true),
                    _ => {}
                }
                if self.order(&k.bytes, key) == Ordering::Greater {
                    return Ok(false);
                }
            }
            if leaf.right == NONE {
                return Ok(false);
            }
            leaf = self.read(leaf.right)?;
        }
    }
}

/// The halves of a full node's entries when the new one at `position` goes
/// in: the upper half gets ⌊n/2⌋ of the n old entries, and the new entry
/// joins the half it belongs in.
fn halve<T: Clone>(entries: &[T], position: usize) -> (Vec<T>, Vec<T>) {
    let mut old: Vec<T> = entries.to_vec();
    let new = old.remove(position);
    let n = old.len();
    let k = n - n / 2;
    let mut hi = old.split_off(k);
    let mut lo = old;
    match position < k {
        true => lo.insert(position, new),
        false => hi.insert(position - k, new),
    }
    (lo, hi)
}

/// A binary order: its bitmap leaves, each covering `capacity` record
/// numbers, under a root that holds the entries of up to [`Bitmap::fanout`]
/// leaves, or of that many interior nodes of as many leaves each.
struct Bitmap<'f> {
    tree: Tree<'f>,
    capacity: u32,
}

/// The entries of a bitmap's interior node: the last record number each
/// child covers and the child's page, with no key between them.
fn bitmap_entries(node: &[u8]) -> Vec<(u32, u32)> {
    let count = usize::from(u16::from_le_bytes([node[2], node[3]]));
    (0..count)
        .map(|i| {
            let at = NODE_HEADER + 8 * i;
            (
                u32::from_be_bytes(node[at..at + 4].try_into().unwrap()),
                u32::from_be_bytes(node[at + 4..at + 8].try_into().unwrap()),
            )
        })
        .collect()
}

/// Puts an entry at the end of a bitmap's interior node.
fn push_bitmap_entry(node: &mut [u8], last: u32, page: u32) {
    let count = usize::from(u16::from_le_bytes([node[2], node[3]]));
    let at = NODE_HEADER + 8 * count;
    node[at..at + 4].copy_from_slice(&last.to_be_bytes());
    node[at + 4..at + 8].copy_from_slice(&page.to_be_bytes());
    node[2..4].copy_from_slice(&(count as u16 + 1).to_le_bytes());
}

impl Bitmap<'_> {
    /// The entries an interior node holds.
    fn fanout(&self) -> usize {
        (self.tree.page_size - NODE_HEADER) / 8
    }

    /// The first record number a bitmap leaf `bytes` covers (big-endian).
    fn first(&self, bytes: &[u8]) -> u32 {
        u32::from_be_bytes(bytes[NODE_HEADER..NODE_HEADER + 4].try_into().unwrap())
    }

    /// The pages from the root down to the leaf that covers `record`, or to
    /// the last leaf, with their bytes.
    fn path(&mut self, record: Option<u32>) -> Result<Vec<(u32, Vec<u8>)>, Error> {
        let mut path = Vec::new();
        let mut page = self.tree.root;
        loop {
            let bytes = self.tree.page(page)?;
            let leaf = u16::from_le_bytes([bytes[0], bytes[1]]) & LEAF != 0;
            let entries = match leaf {
                true => Vec::new(),
                false => bitmap_entries(&bytes),
            };
            path.push((page, bytes));
            if leaf {
                return Ok(path);
            }
            // A bitmap of 512-byte nodes has three levels at most.
            if path.len() > 2 {
                return Err(Error::Corrupt("bitmap too deep"));
            }
            let entry = match record {
                Some(record) => entries.iter().find(|entry| entry.0 >= record),
                None => entries.last(),
            };
            page = entry.ok_or(Error::Corrupt("record past the bitmap"))?.1;
        }
    }

    /// The number of record slots the order has bits for, record 0's
    /// included.
    fn bits(&mut self) -> Result<u32, Error> {
        let path = self.path(None)?;
        let last = &path[path.len() - 1].1;
        Ok(self.first(last) + u32::from(u16::from_le_bytes([last[2], last[3]])))
    }

    fn set_bit(&mut self, record: u32, deleted: bool) -> Result<(), Error> {
        let (page, mut bytes) = self.path(Some(record))?.pop().unwrap();
        let i = (record - self.first(&bytes)) as usize;
        match deleted {
            true => bytes[BITMAP_OFFSET + i / 8] |= 1 << (i % 8),
            false => bytes[BITMAP_OFFSET + i / 8] &= !(1 << (i % 8)),
        }
        self.tree.put(page, &bytes)
    }

    /// Writes a new leaf of one bit, the first of the record numbers from
    /// `first`, and links the leaf `previous` (with its page) to it.
    fn new_leaf(&mut self, previous: (u32, &mut [u8]), first: u32) -> Result<u32, Error> {
        let new = self.tree.new_page()?;
        let mut leaf = vec![0; self.tree.page_size];
        leaf[0..2].copy_from_slice(&LEAF.to_le_bytes());
        leaf[2..4].copy_from_slice(&1u16.to_le_bytes());
        leaf[4..8].copy_from_slice(&previous.0.to_le_bytes());
        leaf[8..12].copy_from_slice(&NONE.to_le_bytes());
        leaf[12..16].copy_from_slice(&first.to_be_bytes());
        self.tree.put(new, &leaf)?;
        previous.1[8..12].copy_from_slice(&new.to_le_bytes());
        self.tree.put(previous.0, previous.1)?;
        Ok(new)
    }

    /// A new interior node's page, written empty so that the next new page
    /// is another.
    fn new_interior(&mut self) -> Result<u32, Error> {
        let page = self.tree.new_page()?;
        let zeros = vec![0; self.tree.page_size];
        self.tree.put(page, &zeros)?;
        Ok(page)
    }

    /// Writes an interior node (not the root) of the entries `entries`,
    /// between its siblings.
    fn put_interior(
        &mut self,
        page: u32,
        entries: &[(u32, u32)],
        previous: u32,
        next: u32,
    ) -> Result<(), Error> {
        let mut node = vec![0; self.tree.page_size];
        node[4..8].copy_from_slice(&previous.to_le_bytes());
        node[8..12].copy_from_slice(&next.to_le_bytes());
        for &(last, child) in entries {
            push_bitmap_entry(&mut node, last, child);
        }
        self.tree.put(page, &node)
    }

    /// An empty root interior node.
    fn new_root(&self) -> Vec<u8> {
        let mut top = vec![0; self.tree.page_size];
        top[0..2].copy_from_slice(&ROOT.to_le_bytes());
        top[4..12].fill(0xFF);
        top
    }

    /// Adds the bit of a new record slot (always the next one).
    fn append(&mut self) -> Result<(), Error> {
        let mut path = self.path(None)?;
        let (last, mut bytes) = path.pop().unwrap();
        let count = u32::from(u16::from_le_bytes([bytes[2], bytes[3]]));
        if count < self.capacity {
            bytes[2..4].copy_from_slice(&((count + 1) as u16).to_le_bytes());
            return self.tree.put(last, &bytes);
        }
        let first = self.first(&bytes) + self.capacity;
        let last_record = first + self.capacity - 1;
        match path.as_slice() {
            [] => {
                // The root leaf splits: it moves to a new page, and the root
                // becomes an interior node.
                let a = self.tree.new_page()?;
                let zeros = vec![0; self.tree.page_size];
                self.tree.put(a, &zeros)?;
                let new = self.tree.new_page()?;
                bytes[0..2].copy_from_slice(&LEAF.to_le_bytes());
                bytes[4..8].copy_from_slice(&NONE.to_le_bytes());
                bytes[8..12].copy_from_slice(&new.to_le_bytes());
                self.tree.put(a, &bytes)?;
                let mut top = self.new_root();
                push_bitmap_entry(&mut top, first - 1, a);
                push_bitmap_entry(&mut top, last_record, new);
                self.tree.put(self.tree.root, &top)?;
                let mut leaf = vec![0; self.tree.page_size];
                leaf[0..2].copy_from_slice(&LEAF.to_le_bytes());
                leaf[2..4].copy_from_slice(&1u16.to_le_bytes());
                leaf[4..8].copy_from_slice(&a.to_le_bytes());
                leaf[8..12].copy_from_slice(&NONE.to_le_bytes());
                leaf[12..16].copy_from_slice(&first.to_be_bytes());
                self.tree.put(new, &leaf)
            }
            [(root, top)] => {
                let mut top = top.clone();
                let new = self.new_leaf((last, bytes.as_mut_slice()), first)?;
                if usize::from(u16::from_le_bytes([top[2], top[3]])) < self.fanout() {
                    push_bitmap_entry(&mut top, last_record, new);
                    return self.tree.put(*root, &top);
                }
                // The root's leaves move under an interior node, and the
                // new leaf starts a second one; the original takes their
                // pages in this order.
                let entries = bitmap_entries(&top);
                let a = self.new_interior()?;
                let b = self.new_interior()?;
                self.put_interior(a, &entries, NONE, b)?;
                self.put_interior(b, &[(last_record, new)], a, NONE)?;
                let mut top = self.new_root();
                push_bitmap_entry(&mut top, first - 1, a);
                push_bitmap_entry(&mut top, last_record, b);
                self.tree.put(*root, &top)
            }
            [(root, top), (interior, node)] => {
                let (mut top, mut node) = (top.clone(), node.clone());
                let roots = usize::from(u16::from_le_bytes([top[2], top[3]]));
                let full = usize::from(u16::from_le_bytes([node[2], node[3]])) >= self.fanout();
                if full && roots >= self.fanout() {
                    return Err(Error::Corrupt("bitmap order needs a third interior level"));
                }
                let new = self.new_leaf((last, bytes.as_mut_slice()), first)?;
                if !full {
                    push_bitmap_entry(&mut node, last_record, new);
                    self.tree.put(*interior, &node)?;
                    // The root's entry for it tells the last record it covers.
                    let at = NODE_HEADER + 8 * (roots - 1);
                    top[at..at + 4].copy_from_slice(&last_record.to_be_bytes());
                    return self.tree.put(*root, &top);
                }
                let b = self.new_interior()?;
                node[8..12].copy_from_slice(&b.to_le_bytes());
                self.tree.put(*interior, &node)?;
                self.put_interior(b, &[(last_record, new)], *interior, NONE)?;
                push_bitmap_entry(&mut top, last_record, b);
                self.tree.put(*root, &top)
            }
            _ => unreachable!("a path has up to three pages"),
        }
    }
}

impl IndexFile {
    fn tree(&mut self, tag: &Tag) -> Tree<'_> {
        let page_size = self.page_size as usize;
        let geometry = Geometry {
            page_size,
            unit: page_size as u64,
        };
        Tree {
            file: &mut self.file,
            geometry,
            page_size,
            header: u64::from(tag.header_page) * page_size as u64,
            root: tag.root,
            key_length: usize::from(tag.key_length),
            pad: Some(tag.pad),
            weights: None,
            aes: match &self.cipher {
                RecordCipher::Aes { key, nonce } => Some((key.clone(), *nonce)),
                _ => None,
            },
            aes_iv: Some(Arc::clone(&self.aes_iv)),
            rc4: match &self.cipher {
                RecordCipher::Rc4(key) => Some(key.clone()),
                _ => None,
            },
            kept: HashMap::new(),
            scored: tag.keeps_score,
            scores: HashMap::new(),
        }
    }

    fn bitmap(&mut self, tag: &Tag) -> Bitmap<'_> {
        let capacity = (self.page_size - BITMAP_OFFSET as u32) * 8;
        Bitmap {
            tree: self.tree(tag),
            capacity,
        }
    }

    /// Counts a record write that changed the order `tag`'s keys.
    pub fn count_change(&mut self, tag: &Tag) {
        *self.changes.entry(tag.header_page).or_default() += 1;
    }

    /// Writes the order `tag`'s count of record writes that changed its keys
    /// into its tag header, where only a reindex leaves it otherwise
    /// (`AdsRecallAllRecords` does, `fixtures/write/append_reuse`).
    pub fn write_changes(&mut self, tag: &Tag) -> Result<(), Error> {
        let count = self.changes_of(tag);
        self.tree(tag).set_header_u32(8, count)
    }

    /// Sets the count of record writes that changed the order `tag`'s keys,
    /// to leave out the ones a recall makes: they put a key back in an
    /// ordinary order without the counter noticing, where a binary order's
    /// bit counts.
    pub fn set_changes_of(&mut self, tag: &Tag, count: u32) {
        self.changes.insert(tag.header_page, count);
    }

    /// How many record writes changed the order `tag`'s keys since the file
    /// was opened.
    pub fn changes_of(&self, tag: &Tag) -> u32 {
        self.changes.get(&tag.header_page).copied().unwrap_or(0)
    }

    /// Inserts `key` of record `record` into the order `tag`.
    pub fn insert_key(&mut self, tag: &Tag, key: &[u8], record: u32) -> Result<(), Error> {
        self.tree(tag).insert(Key {
            bytes: key.to_vec(),
            record,
        })
    }

    /// Inserts `key` of record `record` into the full-text order `tag`,
    /// which keeps scores, with the word's count `score`.
    pub fn insert_scored(
        &mut self,
        tag: &Tag,
        key: &[u8],
        record: u32,
        score: u16,
    ) -> Result<(), Error> {
        let mut tree = self.tree(tag);
        tree.scores.insert((record, key.to_vec()), score);
        tree.insert(Key {
            bytes: key.to_vec(),
            record,
        })
    }

    /// Sets the word's count of `key` of record `record` in the full-text
    /// order `tag`, which keeps scores, to `score`, in its leaf.
    pub fn set_score(
        &mut self,
        tag: &Tag,
        key: &[u8],
        record: u32,
        score: u16,
    ) -> Result<(), Error> {
        let mut tree = self.tree(tag);
        let key = Key {
            bytes: key.to_vec(),
            record,
        };
        let (_, page, leaf) = tree.path(&key)?;
        if !leaf.keys.contains(&key) {
            return Err(Error::Corrupt("no such key in the order"));
        }
        tree.scores.insert((record, key.bytes), score);
        tree.write(page, &leaf)
    }

    /// Deletes `key` of record `record` from the order `tag`.
    pub fn delete_key(&mut self, tag: &Tag, key: &[u8], record: u32) -> Result<(), Error> {
        self.tree(tag).delete(&Key {
            bytes: key.to_vec(),
            record,
        })
    }

    /// Whether the order `tag` has `key` of record `record`, or with no
    /// record of any record.
    pub fn has_key(&mut self, tag: &Tag, key: &[u8], record: Option<u32>) -> Result<bool, Error> {
        self.tree(tag).has(key, record)
    }

    /// The records the order `tag` has `key` for.
    pub fn holders(&mut self, tag: &Tag, key: &[u8]) -> Result<Vec<u32>, Error> {
        self.tree(tag).holders(key)
    }

    /// Sets (`deleted`) or clears record `record`'s bit in the binary order
    /// `tag`, adding the bit of a new record slot first.
    pub fn mark_deleted(&mut self, tag: &Tag, record: u32, deleted: bool) -> Result<(), Error> {
        let mut bitmap = self.bitmap(tag);
        while bitmap.bits()? <= record {
            bitmap.append()?;
        }
        bitmap.set_bit(record, deleted)
    }
}

/// A tag directory, whose keys are the tags' names, open for maintenance:
/// `unit` is the bytes a node number counts (the page size in an ADI file, 1
/// in a CDX file, where it is a byte offset).
fn directory_tree(
    file: &mut File,
    page_size: usize,
    unit: u64,
    root: u32,
    key_length: usize,
) -> Tree<'_> {
    Tree {
        file,
        geometry: Geometry { page_size, unit },
        page_size,
        // The file's header, whose second word heads the free list.
        header: 0,
        root,
        key_length,
        weights: None,
        pad: Some(b' '),
        aes: None,
        rc4: None,
        aes_iv: None,
        kept: HashMap::new(),
        scored: false,
        scores: HashMap::new(),
    }
}

/// The entries of a tag directory whose root is not a leaf, in order: the
/// leaves are read from the first one on, along their right siblings.
pub(crate) fn directory_keys(
    file: &mut File,
    page_size: usize,
    unit: u64,
    root: u32,
    key_length: usize,
) -> Result<Vec<Key>, Error> {
    let mut tree = directory_tree(file, page_size, unit, root, key_length);
    let mut node = tree.read(root)?;
    for _ in 0..32 {
        if node.leaf {
            break;
        }
        let first = *node
            .children
            .first()
            .ok_or(Error::Corrupt("empty interior node"))?;
        node = tree.read(first)?;
    }
    let mut keys = Vec::new();
    // A directory of 256 tags has a few dozen leaves at most.
    for _ in 0..10_000 {
        keys.extend(node.keys.iter().cloned());
        if node.right == NONE {
            return Ok(keys);
        }
        node = tree.read(node.right)?;
    }
    Err(Error::Corrupt("tag directory loops"))
}

/// Adds an entry to a tag directory, splitting its nodes as an order's tree
/// does (`fixtures/write/tagdir`).
pub(crate) fn directory_insert(
    file: &mut File,
    page_size: usize,
    unit: u64,
    root: u32,
    key_length: usize,
    key: Key,
) -> Result<(), Error> {
    directory_tree(file, page_size, unit, root, key_length).insert(key)
}

/// Removes an entry from a tag directory.
pub(crate) fn directory_delete(
    file: &mut File,
    page_size: usize,
    unit: u64,
    root: u32,
    key_length: usize,
    key: &Key,
) -> Result<(), Error> {
    directory_tree(file, page_size, unit, root, key_length).delete(key)
}

/// Where a CDX or IDX file keeps the change counter (big-endian).
const CDX_COUNTER: u64 = 0x0A;

impl CdxFile {
    fn tree(&mut self, tag: &Tag) -> Tree<'_> {
        let geometry = Geometry {
            page_size: crate::cdx::NODE_SIZE,
            unit: 1,
        };
        Tree {
            // Only text keys are weighed (see [`CdxFile::set_weights`]).
            weights: self.weights.as_deref().copied().filter(|_| tag.pad == b' '),
            file: &mut self.file,
            geometry,
            page_size: geometry.page_size,
            header: u64::from(tag.header_page),
            root: tag.root,
            key_length: usize::from(tag.key_length),
            // Orders with a condition have no trailing compression.
            pad: tag.condition.is_empty().then_some(tag.pad),
            // CDX/IDX files are never AES-encrypted (that is an ADI concern).
            aes: None,
            rc4: None,
            aes_iv: None,
            kept: HashMap::new(),
            scored: tag.keeps_score,
            scores: HashMap::new(),
        }
    }

    fn bitmap(&mut self, tag: &Tag) -> Bitmap<'_> {
        let capacity = (crate::cdx::NODE_SIZE - BITMAP_OFFSET) as u32 * 8;
        Bitmap {
            tree: self.tree(tag),
            capacity,
        }
    }

    /// Inserts `key` of record `record` into the order `tag`.
    pub fn insert_key(&mut self, tag: &Tag, key: &[u8], record: u32) -> Result<(), Error> {
        self.tree(tag).insert(Key {
            bytes: key.to_vec(),
            record,
        })
    }

    /// Inserts `key` of record `record` into the full-text order `tag`, which
    /// keeps scores, with the word's count `score`.
    pub fn insert_scored(
        &mut self,
        tag: &Tag,
        key: &[u8],
        record: u32,
        score: u16,
    ) -> Result<(), Error> {
        let mut tree = self.tree(tag);
        tree.scores.insert((record, key.to_vec()), score);
        tree.insert(Key {
            bytes: key.to_vec(),
            record,
        })
    }

    /// Sets the word's count of `key` of record `record` in the full-text
    /// order `tag`, which keeps scores, to `score`, in its leaf.
    pub fn set_score(
        &mut self,
        tag: &Tag,
        key: &[u8],
        record: u32,
        score: u16,
    ) -> Result<(), Error> {
        let mut tree = self.tree(tag);
        let key = Key {
            bytes: key.to_vec(),
            record,
        };
        let (_, page, leaf) = tree.path(&key)?;
        if !leaf.keys.contains(&key) {
            return Err(Error::Corrupt("no such key in the order"));
        }
        tree.scores.insert((record, key.bytes), score);
        tree.write(page, &leaf)
    }

    /// Deletes `key` of record `record` from the order `tag`, if it has it;
    /// whether it had.
    pub fn delete_key(&mut self, tag: &Tag, key: &[u8], record: u32) -> Result<bool, Error> {
        let mut tree = self.tree(tag);
        if !tree.has(key, Some(record))? {
            return Ok(false);
        }
        tree.delete(&Key {
            bytes: key.to_vec(),
            record,
        })?;
        Ok(true)
    }

    /// Whether the order `tag` has `key` of record `record`, or with no
    /// record of any record.
    pub fn has_key(&mut self, tag: &Tag, key: &[u8], record: Option<u32>) -> Result<bool, Error> {
        self.tree(tag).has(key, record)
    }

    /// The records the order `tag` has `key` for.
    pub fn holders(&mut self, tag: &Tag, key: &[u8]) -> Result<Vec<u32>, Error> {
        self.tree(tag).holders(key)
    }

    /// Sets or clears record `record`'s bit in the binary order `tag` (set:
    /// its key is `T`), adding the bits of new record slots first.
    pub fn set_bit(&mut self, tag: &Tag, record: u32, set: bool) -> Result<(), Error> {
        let mut bitmap = self.bitmap(tag);
        while bitmap.bits()? <= record {
            bitmap.append()?;
        }
        bitmap.set_bit(record, set)
    }

    /// Whether the binary order `tag` has a bit for record `record`.
    pub fn has_bit(&mut self, tag: &Tag, record: u32) -> Result<bool, Error> {
        Ok(self.bitmap(tag).bits()? > record)
    }

    /// Adds `n` to the file's change counter, which wraps at 16 bits.
    pub fn count_changes(&mut self, n: u16) -> Result<(), Error> {
        let mut bytes = [0; 2];
        self.file.seek(SeekFrom::Start(CDX_COUNTER))?;
        self.file.read_exact(&mut bytes)?;
        let counter = u16::from_be_bytes(bytes).wrapping_add(n);
        self.file.seek(SeekFrom::Start(CDX_COUNTER))?;
        Ok(self.file.write_all(&counter.to_be_bytes())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widened_layouts() {
        // The first widening of each key length's first layout: 1-byte keys
        // from 14 bits to 22 at record 16,384, 2-byte keys from 12 to 20,
        // 4-byte keys from 18 to 26 at 262,144, 8-byte keys from 16 to 24 at
        // 65,536.
        assert_eq!(Layout::widened(1, 16_384).r, 22);
        assert_eq!(Layout::widened(2, 16_384).r, 20);
        assert_eq!(Layout::widened(4, 262_144).r, 26);
        assert_eq!(
            Layout::widened(8, 65_536),
            Layout {
                r: 24,
                d: 4,
                t: 4,
                s: 4
            }
        );
    }

    #[test]
    fn halves() {
        // Five old entries and a new one at 2: the upper half gets two.
        let (lo, hi) = halve(&[1, 2, 9, 3, 4, 5], 2);
        assert_eq!((lo, hi), (vec![1, 2, 9, 3], vec![4, 5]));
        let (lo, hi) = halve(&[1, 2, 3, 4, 9, 5], 4);
        assert_eq!((lo, hi), (vec![1, 2, 3], vec![4, 9, 5]));
    }
}
