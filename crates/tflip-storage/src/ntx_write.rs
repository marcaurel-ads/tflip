//! Keeping an NTX file current as records change, as the original engine
//! does (`fixtures/write/ntx_indexes`): keys inserted into and deleted from
//! its B-tree in place.
//!
//! A page's items are reached through its offset table and stay in their
//! slots; inserting or removing a key copies the items after it one slot
//! along, and a slot an item leaves is zeroed (its record and key; a child
//! pointer is cleared). An insert goes after the keys equal to it, into a
//! leaf. A full page splits: of its keys and the new one, the first *half*
//! (header 0x14) stay, the next moves up into the parent, the rest go to a
//! new page; a split root gets a new root above it. A delete takes a key
//! from an interior page's place its predecessor, the last key of the
//! rightmost leaf below it. A page left with fewer than *half* keys takes the
//! key that separates it from its sibling down from the parent, and the two
//! are merged when their keys fit one page (the right one freed), else
//! shared out evenly with the left page's last key going up as the new
//! separator. A root left without keys gives way to its only child. Freed
//! pages form a list headed by header 0x08, linked through the child pointer
//! of their first item; new pages come from its head, else the end of the
//! file. The header's change counter (0x02) counts every key inserted or
//! deleted; a `UNIQUE` order skips a key that is there already, uncounted.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};

use crate::ntx::{NtxFile, PAGE_SIZE};
use crate::order::Error;

/// Where the header keeps what changes.
const COUNTER: usize = 2;
const ROOT: usize = 4;
const FREE: usize = 8;
const MAX_KEYS: usize = 0x12;
const HALF: usize = 0x14;

/// An item to place: its record, key and the child page right of it.
#[derive(Debug, Clone)]
struct Item {
    record: u32,
    key: Vec<u8>,
    right: u32,
}

/// One step of a path from the root: a page, a position in it, and its key
/// count when the step was taken.
#[derive(Debug, Clone, Copy)]
struct Step {
    page: u32,
    pos: usize,
    count: usize,
}

/// The pages of an NTX file that one change reads and writes, kept until
/// it is done.
struct Pages<'f> {
    file: &'f mut NtxFile,
    pages: HashMap<u32, Box<[u8; PAGE_SIZE]>>,
    dirty: Vec<u32>,
    end: u64,
    key_length: usize,
    max: usize,
    half: usize,
    root: u32,
    free: u32,
    counter: u16,
}

impl<'f> Pages<'f> {
    fn new(file: &'f mut NtxFile) -> Result<Pages<'f>, Error> {
        let mut header = [0; 32];
        file.file.seek(SeekFrom::Start(0))?;
        file.file
            .read_exact(&mut header)
            .map_err(|_| Error::Corrupt("NTX file too short"))?;
        let u16_at = |at: usize| usize::from(u16::from_le_bytes([header[at], header[at + 1]]));
        let u32_at = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
        let end = file.file.seek(SeekFrom::End(0))?;
        let key_length = usize::from(file.header.key_length);
        let max = u16_at(MAX_KEYS);
        if max == 0 || 2 + 2 * (max + 1) + (max + 1) * (key_length + 8) > PAGE_SIZE {
            return Err(Error::Corrupt("NTX page size does not fit its keys"));
        }
        Ok(Pages {
            key_length,
            max,
            half: u16_at(HALF),
            root: u32_at(ROOT),
            free: u32_at(FREE),
            counter: u16_at(COUNTER) as u16,
            file,
            pages: HashMap::new(),
            dirty: Vec::new(),
            end,
        })
    }

    fn page(&mut self, page: u32) -> Result<&mut [u8; PAGE_SIZE], Error> {
        if !self.pages.contains_key(&page) {
            if page == 0 || !(page as usize).is_multiple_of(PAGE_SIZE) {
                return Err(Error::Corrupt("NTX page offset not on a page"));
            }
            let mut bytes = Box::new([0; PAGE_SIZE]);
            self.file.file.seek(SeekFrom::Start(u64::from(page)))?;
            self.file
                .file
                .read_exact(&mut bytes[..])
                .map_err(|_| Error::Corrupt("NTX file ends inside a page"))?;
            self.pages.insert(page, bytes);
        }
        Ok(self.pages.get_mut(&page).expect("a page read"))
    }

    fn touch(&mut self, page: u32) {
        if !self.dirty.contains(&page) {
            self.dirty.push(page);
        }
    }

    /// Writes the pages changed and the header's counter, root and free list.
    fn finish(mut self) -> Result<(), Error> {
        self.dirty.sort_unstable();
        for page in std::mem::take(&mut self.dirty) {
            let bytes = &self.pages[&page];
            self.file.file.seek(SeekFrom::Start(u64::from(page)))?;
            self.file.file.write_all(&bytes[..])?;
        }
        let mut header = [0; 10];
        header[..2].copy_from_slice(&self.counter.to_le_bytes());
        header[2..6].copy_from_slice(&self.root.to_le_bytes());
        header[6..].copy_from_slice(&self.free.to_le_bytes());
        self.file.file.seek(SeekFrom::Start(COUNTER as u64))?;
        self.file.file.write_all(&header)?;
        self.file.header.root = self.root;
        Ok(())
    }

    fn u16_at(&mut self, page: u32, at: usize) -> Result<usize, Error> {
        let bytes = self.page(page)?;
        Ok(usize::from(u16::from_le_bytes([bytes[at], bytes[at + 1]])))
    }

    /// Where item `i` of `page` starts in it.
    fn slot(&mut self, page: u32, i: usize) -> Result<usize, Error> {
        if i > self.max {
            return Err(Error::Corrupt("NTX item beyond the page's maximum"));
        }
        let at = self.u16_at(page, 2 + 2 * i)?;
        if at + self.key_length + 8 > PAGE_SIZE {
            return Err(Error::Corrupt("NTX item outside its page"));
        }
        Ok(at)
    }

    fn count(&mut self, page: u32) -> Result<usize, Error> {
        self.u16_at(page, 0)
    }

    fn set_count(&mut self, page: u32, n: usize) -> Result<(), Error> {
        self.page(page)?[..2].copy_from_slice(&(n as u16).to_le_bytes());
        self.touch(page);
        Ok(())
    }

    fn child(&mut self, page: u32, i: usize) -> Result<u32, Error> {
        let at = self.slot(page, i)?;
        let bytes = self.page(page)?;
        Ok(u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()))
    }

    fn set_child(&mut self, page: u32, i: usize, child: u32) -> Result<(), Error> {
        let at = self.slot(page, i)?;
        self.page(page)?[at..at + 4].copy_from_slice(&child.to_le_bytes());
        self.touch(page);
        Ok(())
    }

    /// The record and key of item `i` of `page`.
    fn entry(&mut self, page: u32, i: usize) -> Result<(u32, Vec<u8>), Error> {
        let at = self.slot(page, i)?;
        let length = self.key_length;
        let bytes = self.page(page)?;
        let record = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap());
        Ok((record, bytes[at + 8..at + 8 + length].to_vec()))
    }

    fn key(&mut self, page: u32, i: usize) -> Result<Vec<u8>, Error> {
        Ok(self.entry(page, i)?.1)
    }

    fn set_entry(&mut self, page: u32, i: usize, record: u32, key: &[u8]) -> Result<(), Error> {
        let at = self.slot(page, i)?;
        let bytes = self.page(page)?;
        bytes[at + 4..at + 8].copy_from_slice(&record.to_le_bytes());
        bytes[at + 8..at + 8 + key.len()].copy_from_slice(key);
        self.touch(page);
        Ok(())
    }

    fn zero_entry(&mut self, page: u32, i: usize) -> Result<(), Error> {
        let at = self.slot(page, i)?;
        let length = self.key_length;
        self.page(page)?[at + 4..at + 8 + length].fill(0);
        self.touch(page);
        Ok(())
    }

    /// Copies the record and key of item `from` of `source` to item `to` of
    /// `page`.
    fn copy_entry(&mut self, page: u32, to: usize, source: u32, from: usize) -> Result<(), Error> {
        let (record, key) = self.entry(source, from)?;
        self.set_entry(page, to, record, &key)
    }

    fn copy_child(&mut self, page: u32, to: usize, source: u32, from: usize) -> Result<(), Error> {
        let child = self.child(source, from)?;
        self.set_child(page, to, child)
    }

    fn copy_item(&mut self, page: u32, to: usize, source: u32, from: usize) -> Result<(), Error> {
        self.copy_child(page, to, source, from)?;
        self.copy_entry(page, to, source, from)
    }

    /// A page for a split or a new root: the head of the free list, else one
    /// appended to the file with the offset table of a new page.
    fn new_page(&mut self) -> Result<(u32, bool), Error> {
        if self.free != 0 {
            let page = self.free;
            self.free = self.child(page, 0)?;
            return Ok((page, true));
        }
        let page = u32::try_from(self.end).map_err(|_| Error::Corrupt("NTX file too large"))?;
        self.end += PAGE_SIZE as u64;
        let mut bytes = Box::new([0; PAGE_SIZE]);
        let base = 2 * (self.max + 2);
        for i in 0..=self.max {
            let at = 2 + 2 * i;
            let slot = (base + i * (self.key_length + 8)) as u16;
            bytes[at..at + 2].copy_from_slice(&slot.to_le_bytes());
        }
        self.pages.insert(page, bytes);
        self.touch(page);
        Ok((page, false))
    }

    /// Puts `page` on the free list.
    fn free_page(&mut self, page: u32) -> Result<(), Error> {
        let next = self.free;
        self.set_child(page, 0, next)?;
        self.free = page;
        Ok(())
    }

    /// How two keys compare in the order: by the file's weights, reversed in
    /// a descending order.
    fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
        let order = self.file.compare(a, b);
        if self.file.header.descending {
            order.reverse()
        } else {
            order
        }
    }

    /// Inserts `key` of `record`: into a leaf, after the keys equal to it.
    fn add(&mut self, key: &[u8], record: u32) -> Result<(), Error> {
        let mut path = Vec::new();
        let mut page = self.root;
        loop {
            let n = self.count(page)?;
            let mut pos = n;
            for i in 0..n {
                let stored = self.key(page, i)?;
                if self.compare(&stored, key) == Ordering::Greater {
                    pos = i;
                    break;
                }
            }
            path.push(Step {
                page,
                pos,
                count: n,
            });
            let child = self.child(page, pos)?;
            if child == 0 {
                break;
            }
            if path.len() > crate::ntx::MAX_DEPTH {
                return Err(Error::Corrupt("NTX tree too deep"));
            }
            page = child;
        }
        let depth = path.len() - 1;
        self.insert_item(
            &path,
            depth,
            Item {
                record,
                key: key.to_vec(),
                right: 0,
            },
        )
    }

    /// Places `item` at the position `path[depth]` says, splitting pages up
    /// the path as they fill.
    fn insert_item(&mut self, path: &[Step], depth: usize, mut item: Item) -> Result<(), Error> {
        let mut depth = Some(depth);
        while let Some(d) = depth {
            let Step { page, pos, .. } = path[d];
            if !self.page_insert(page, pos, &mut item)? {
                return Ok(());
            }
            let (new, reused) = self.new_page()?;
            if !reused {
                self.set_count(new, 0)?;
            }
            self.set_child(new, 0, item.right)?;
            let moved = self.count(page)? - self.half;
            for i in 0..moved {
                self.copy_entry(new, i, page, self.half + i)?;
                self.zero_entry(page, self.half + i)?;
                self.copy_child(new, i + 1, page, self.half + i + 1)?;
                self.set_child(page, self.half + i + 1, 0)?;
            }
            item.right = new;
            let n = self.count(page)?;
            self.set_count(page, n - moved)?;
            self.set_count(new, moved)?;
            depth = d.checked_sub(1);
        }
        let (root, _) = self.new_page()?;
        let old = self.root;
        self.set_child(root, 0, old)?;
        self.set_entry(root, 0, item.record, &item.key)?;
        self.set_child(root, 1, item.right)?;
        self.set_count(root, 1)?;
        self.root = root;
        Ok(())
    }

    /// Inserts `item` at `pos` of `page`. Whether the page was full: `item`
    /// is then the one to move up (its right child the new page's first).
    fn page_insert(&mut self, page: u32, pos: usize, item: &mut Item) -> Result<bool, Error> {
        let (n, half) = (self.count(page)?, self.half);
        if n != self.max {
            if pos > n {
                // After the last key (from sharing keys out).
                self.set_entry(page, pos - 1, item.record, &item.key)?;
                self.set_child(page, pos, item.right)?;
            } else {
                if n != 0 {
                    self.copy_child(page, n + 1, page, n)?;
                    for i in (pos..n).rev() {
                        self.copy_item(page, i + 1, page, i)?;
                    }
                }
                self.set_entry(page, pos, item.record, &item.key)?;
                self.set_child(page, pos + 1, item.right)?;
            }
            self.set_count(page, n + 1)?;
            return Ok(false);
        }
        if pos == half {
            // The new key itself moves up.
            return Ok(true);
        }
        let (median, kept) = if pos < half {
            let median = self.entry(page, half - 1)?;
            let kept = self.child(page, half)?;
            let mut d = half;
            for _ in 0..half - pos - 1 {
                self.copy_child(page, d, page, d - 1)?;
                d -= 1;
                self.copy_entry(page, d, page, d - 1)?;
            }
            self.set_entry(page, pos, item.record, &item.key)?;
            self.set_child(page, pos + 1, item.right)?;
            (median, kept)
        } else {
            let median = self.entry(page, half)?;
            let kept = self.child(page, half + 1)?;
            let mut d = half;
            for _ in 0..pos - 1 - half {
                self.copy_entry(page, d, page, d + 1)?;
                d += 1;
                self.copy_child(page, d, page, d + 1)?;
            }
            self.set_entry(page, pos - 1, item.record, &item.key)?;
            self.set_child(page, pos, item.right)?;
            (median, kept)
        };
        *item = Item {
            record: median.0,
            key: median.1,
            right: kept,
        };
        Ok(true)
    }

    /// The path from the root to the key `key` of `record`, if the order has
    /// it.
    fn find(&mut self, key: &[u8], record: u32) -> Result<Option<Vec<Step>>, Error> {
        let mut path = Vec::new();
        let root = self.root;
        self.visit(root, key, record, &mut path, 0)
    }

    fn visit(
        &mut self,
        page: u32,
        key: &[u8],
        record: u32,
        path: &mut Vec<Step>,
        depth: usize,
    ) -> Result<Option<Vec<Step>>, Error> {
        if depth > crate::ntx::MAX_DEPTH {
            return Err(Error::Corrupt("NTX tree too deep"));
        }
        let n = self.count(page)?;
        for i in 0..=n {
            let low = match i {
                0 => None,
                _ => Some(self.key(page, i - 1)?),
            };
            let high = match i < n {
                true => Some(self.key(page, i)?),
                false => None,
            };
            let child = self.child(page, i)?;
            let not_after =
                |pages: &Self, a: &[u8], b: &[u8]| pages.compare(a, b) != Ordering::Greater;
            let in_range = low.as_deref().is_none_or(|low| not_after(self, low, key))
                && high
                    .as_deref()
                    .is_none_or(|high| not_after(self, key, high));
            if child != 0 && in_range {
                path.push(Step {
                    page,
                    pos: i,
                    count: n,
                });
                if let Some(found) = self.visit(child, key, record, path, depth + 1)? {
                    return Ok(Some(found));
                }
                path.pop();
            }
            if i < n && high.as_deref() == Some(key) && self.entry(page, i)?.0 == record {
                path.push(Step {
                    page,
                    pos: i,
                    count: n,
                });
                return Ok(Some(path.clone()));
            }
            // Past the key: it is not there.
            if let Some(high) = &high
                && !not_after(self, high, key)
            {
                return Ok(None);
            }
        }
        Ok(None)
    }

    /// Deletes `key` of `record`; whether the order had it.
    fn remove(&mut self, key: &[u8], record: u32) -> Result<bool, Error> {
        let Some(mut path) = self.find(key, record)? else {
            return Ok(false);
        };
        let Step {
            mut page, mut pos, ..
        } = *path.last().expect("a path");
        let child = self.child(page, pos)?;
        if child != 0 {
            // An interior key: its predecessor takes its place.
            let mut q = child;
            let m = loop {
                let m = self.count(q)?;
                let next = self.child(q, m)?;
                if next == 0 {
                    path.push(Step {
                        page: q,
                        pos: m - 1,
                        count: m,
                    });
                    break m;
                }
                path.push(Step {
                    page: q,
                    pos: m,
                    count: m,
                });
                q = next;
            };
            self.copy_entry(page, pos, q, m - 1)?;
            page = q;
            pos = m - 1;
        }
        self.remove_key(page, pos)?;
        let depth = path.len() - 1;
        self.fix_underflow(&mut path, depth)?;
        Ok(true)
    }

    /// Removes key `pos` of `page` and the child right of it; returns its
    /// record and key.
    fn remove_key(&mut self, page: u32, pos: usize) -> Result<(u32, Vec<u8>), Error> {
        let n = self.count(page)?;
        let out;
        if pos == n {
            out = self.entry(page, pos - 1)?;
            self.zero_entry(page, pos - 1)?;
            self.set_child(page, pos, 0)?;
        } else {
            out = self.entry(page, pos)?;
            let mut d = pos;
            for _ in 0..n - pos - 1 {
                self.copy_entry(page, d, page, d + 1)?;
                d += 1;
                self.copy_child(page, d, page, d + 1)?;
            }
            self.set_child(page, d + 1, 0)?;
            self.zero_entry(page, d)?;
        }
        self.set_count(page, n - 1)?;
        Ok(out)
    }

    fn fix_underflow(&mut self, path: &mut [Step], mut depth: usize) -> Result<(), Error> {
        loop {
            let page = path[depth].page;
            if self.count(page)? >= self.half {
                return Ok(());
            }
            if depth == 0 {
                if self.count(page)? == 0 && self.child(page, 0)? != 0 {
                    self.root = self.child(page, 0)?;
                    self.free_page(page)?;
                }
                return Ok(());
            }
            depth -= 1;
            let Step {
                page: q,
                pos,
                count,
            } = path[depth];
            let (left, right) = match pos == count {
                true => (self.child(q, pos - 1)?, self.child(q, pos)?),
                false => (self.child(q, pos)?, self.child(q, pos + 1)?),
            };
            let separator = self.remove_key(q, pos)?;
            let ln = self.count(left)?;
            if ln < self.max {
                // The separator goes to the end of the left page,
                self.set_entry(left, ln, separator.0, &separator.1)?;
                self.set_count(left, ln + 1)?;
            } else {
                // or to the front of the right one.
                let rn = self.count(right)?;
                self.copy_child(right, rn + 1, right, rn)?;
                for i in (0..rn).rev() {
                    self.copy_item(right, i + 1, right, i)?;
                }
                self.set_count(right, rn + 1)?;
                self.set_entry(right, 0, separator.0, &separator.1)?;
                self.copy_child(right, 0, left, ln)?;
            }
            if self.count(left)? + self.count(right)? <= self.max {
                self.merge(left, right)?;
                continue;
            }
            self.balance(left, right)?;
            let ln = self.count(left)? - 1;
            self.set_count(left, ln)?;
            let (record, key) = self.entry(left, ln)?;
            return self.insert_item(path, depth, Item { record, key, right });
        }
    }

    fn merge(&mut self, left: u32, right: u32) -> Result<(), Error> {
        let (mut si, rn) = (self.count(left)?, self.count(right)?);
        for i in 0..rn {
            self.copy_item(left, si, right, i)?;
            si += 1;
        }
        let ln = self.count(left)?;
        self.set_count(left, ln + rn)?;
        self.set_count(right, 0)?;
        self.copy_child(left, si, right, rn)?;
        self.free_page(right)
    }

    fn balance(&mut self, left: u32, right: u32) -> Result<(), Error> {
        let (ln, rn) = (self.count(left)?, self.count(right)?);
        match rn.cmp(&ln) {
            Ordering::Greater => {
                let n = (rn - ln).div_ceil(2);
                for i in 0..n {
                    self.copy_item(left, ln + i, right, i)?;
                }
                self.set_count(left, ln + n)?;
                for i in 0..rn - n {
                    self.copy_item(right, i, right, i + n)?;
                }
                self.copy_child(right, rn - n, right, rn)?;
                self.set_count(right, rn - n)?;
            }
            Ordering::Less => {
                let n = (1 + ln - rn) / 2 - 1;
                self.copy_child(right, rn + n, right, rn)?;
                for k in 0..rn {
                    self.copy_item(right, rn + n - 1 - k, right, rn - 1 - k)?;
                }
                for k in 0..n {
                    self.copy_item(right, n - 1 - k, left, ln - 1 - k)?;
                }
                self.set_count(right, rn + n)?;
                self.set_count(left, ln - n)?;
            }
            Ordering::Equal => {}
        }
        Ok(())
    }

    /// Whether any record has `key`.
    fn present(&mut self, page: u32, key: &[u8], depth: usize) -> Result<bool, Error> {
        if depth > crate::ntx::MAX_DEPTH {
            return Err(Error::Corrupt("NTX tree too deep"));
        }
        let n = self.count(page)?;
        for i in 0..=n {
            let child = self.child(page, i)?;
            if child != 0 && self.present(child, key, depth + 1)? {
                return Ok(true);
            }
            if i < n && self.key(page, i)? == key {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn bump(&mut self) {
        self.counter = self.counter.wrapping_add(1);
    }
}

impl NtxFile {
    /// Inserts the key `key` of record `record`, counting it; a `UNIQUE`
    /// order keeps only the first record with a key, and skips it uncounted
    /// when it has it.
    pub fn insert_key(&mut self, key: &[u8], record: u32) -> Result<(), Error> {
        let mut pages = Pages::new(self)?;
        let root = pages.root;
        if pages.file.header.unique && pages.present(root, key, 0)? {
            return Ok(());
        }
        pages.add(key, record)?;
        pages.bump();
        pages.finish()
    }

    /// Deletes the key `key` of record `record`, counting it whether or not
    /// the order had it; whether it had.
    pub fn delete_key(&mut self, key: &[u8], record: u32) -> Result<bool, Error> {
        let mut pages = Pages::new(self)?;
        let found = pages.remove(key, record)?;
        pages.bump();
        pages.finish()?;
        Ok(found)
    }
}
