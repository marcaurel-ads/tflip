//! Writing ADT memo files (`.adm`): where values go, as the original
//! engine's allocator places them, so that the files it writes are the
//! engine's byte for byte (`fixtures/write/memos`).
//!
//! The first 4,096 bytes are the header. Sizes are counted in blocks
//! throughout, and integers are little-endian except the two big-endian
//! ones FPT files have at 0x00 and 0x06:
//!
//! | Offset | Size | Meaning |
//! | --- | --- | --- |
//! | 0x00 | u32 BE | the end of the allocated space: where the file grows |
//! | 0x06 | u16 BE | the block size in bytes |
//! | 0x08 | u16 | the number of fragments |
//! | 0x0D | u8 | allocations made (rewrites in place not counted), up to 255 |
//! | 0x10 | 8 × *n* | fragments: (size, first block), free runs too small for a class |
//! | 0x400 | 128 × 8 | size classes: (smallest run of the class, first free run) |
//! | 0x800 | 32 × 8 | slabs: (chunk size 1–32, first slab with a free chunk) |
//!
//! Values of up to 32 blocks go in *slabs*: 128-aligned spans of 129 blocks,
//! whose first block(s) hold a 24-byte record (the marker 0xFB, the chunk
//! count, the chunks in use, the chunk size, the next slab of the list, and a
//! bitmap of the chunks in use) and the rest chunks of one size. Larger
//! values are *extents*, rounded up to the largest size of their size class.
//! A free run's first block holds its size and the next run of its class's
//! list (behind an 8-byte IV, zero, in an AES table's or dictionary's file). Freed space is not coalesced, but for the merge of a run with the
//! fragments next to it; some is never freed at all (a value set to NULL, the
//! last chunk of a slab that is not the head of its list), as with the
//! original engine.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::source::Source;
use crate::table::Error;

const FRAGMENTS: u64 = 0x10;
const CLASSES: u64 = 0x400;
const CLASS_COUNT: usize = 128;
const SLABS: u64 = 0x800;
/// The largest value kept in a slab, in blocks.
pub(crate) const MAX_CHUNK: u32 = 32;
/// Slabs start at multiples of this many blocks...
const SLAB_ALIGN: u32 = 128;
/// ... and take this many in the allocator's accounting.
const SLAB_SPAN: u32 = 129;
/// The size of a slab's record: an 8-byte header and a 16-byte bitmap.
const META_BYTES: u32 = 24;
const MARKER: u8 = 0xFB;
/// The room the header has for fragments, between 0x10 and 0x400.
const MAX_FRAGMENTS: usize = 126;

/// How a memo file stores its slabs' records: as they are, or, in an AES
/// table's or dictionary's, behind an 8-byte IV, as the original stores them
/// (`fixtures/aes/dictwrite`) — zero and the record clear in one that keeps
/// its records in the clear, fresh from the IV counter and the record
/// enciphered (with the key and nonce) in one that encrypts them. A table
/// that was decrypted still has records enciphered: `Clear` has their key
/// and nonce, to read them with.
#[derive(Clone, Default)]
pub(crate) enum SlabRecords {
    #[default]
    Plain,
    Clear(Option<(Vec<u8>, [u8; 4])>),
    Enciphered {
        key: Vec<u8>,
        nonce: [u8; 4],
        counter: Arc<AtomicU64>,
    },
}

impl SlabRecords {
    /// The key and nonce records are read with, if any are enciphered.
    fn key(&self) -> Option<(&[u8], &[u8; 4])> {
        match self {
            SlabRecords::Enciphered { key, nonce, .. } => Some((key, nonce)),
            SlabRecords::Clear(Some((key, nonce))) => Some((key, nonce)),
            _ => None,
        }
    }

    /// The bytes before a slab's record.
    fn prefix(&self) -> u32 {
        match self {
            SlabRecords::Plain => 0,
            _ => 8,
        }
    }
}

/// The allocator of a memo file, working on the file itself.
pub(crate) struct Allocator<'f> {
    file: &'f mut Source,
    /// The block size in bytes.
    bs: u32,
    /// The smallest run of each size class.
    class_min: [u32; CLASS_COUNT],
    /// The blocks a slab's record takes.
    meta_blocks: u32,
    /// How a slab's record is stored (see [`SlabRecords`]).
    records: SlabRecords,
    /// Whether allocations count in the header (not in a transaction).
    counting: bool,
}

/// Where a free run's list leads on: from the head of its class (the offset
/// in the header) or from the run before it.
#[derive(Clone, Copy)]
enum Link {
    Head(u64),
    Run(u32),
}

/// The offset of size class `i`'s list head.
fn class_head(i: usize) -> u64 {
    CLASSES + 8 * i as u64 + 4
}

/// The offset of the list head of the slabs with chunks of `c` blocks.
fn slab_head(c: u32) -> u64 {
    SLABS + 8 * u64::from(c - 1) + 4
}

impl<'f> Allocator<'f> {
    /// The allocator of the memo file `file` with blocks of `bs` bytes,
    /// counting its allocations in the header if `counting`.
    pub(crate) fn new(
        file: &'f mut Source,
        bs: u32,
        counting: bool,
        records: SlabRecords,
    ) -> Result<Allocator<'f>, Error> {
        let class_min = [0; CLASS_COUNT];
        let meta_blocks = (records.prefix() + META_BYTES).div_ceil(bs);
        let mut allocator = Allocator {
            file,
            bs,
            class_min,
            meta_blocks,
            records,
            counting,
        };
        for i in 0..CLASS_COUNT {
            allocator.class_min[i] = allocator.u32(CLASSES + 8 * i as u64)?;
        }
        Ok(allocator)
    }

    /// The record of the slab at `slab`: its header (marker, chunks, chunks
    /// in use, chunk size, the next slab of its list) and its bitmap.
    fn slab_record(&mut self, slab: u32) -> io::Result<[u8; META_BYTES as usize]> {
        let at = self.at(slab);
        let prefix = self.records.prefix() as usize;
        let mut stored = vec![0; prefix + META_BYTES as usize];
        self.file.read_at(at, &mut stored)?;
        let mut record: [u8; META_BYTES as usize] = stored[prefix..].try_into().unwrap();
        // A record written before the table was encrypted is in the clear,
        // behind a zero IV, and stays so until it is written again.
        if let Some((key, nonce)) = self.records.key() {
            let iv: [u8; 8] = stored[..8].try_into().unwrap();
            if iv != [0; 8] {
                crate::crypto::aes_ctr_xor(key, &iv, nonce, &mut record);
            }
        }
        Ok(record)
    }

    /// Stores `record` as the slab at `slab`'s.
    fn set_slab_record(&mut self, slab: u32, record: &[u8; META_BYTES as usize]) -> io::Result<()> {
        let mut stored = Vec::with_capacity(8 + record.len());
        match &self.records {
            SlabRecords::Plain => stored.extend_from_slice(record),
            SlabRecords::Clear(_) => {
                stored.extend_from_slice(&[0; 8]);
                stored.extend_from_slice(record);
            }
            SlabRecords::Enciphered {
                key,
                nonce,
                counter,
            } => {
                let iv = counter.fetch_add(1, Ordering::Relaxed).to_le_bytes();
                let mut record = *record;
                crate::crypto::aes_ctr_xor(key, &iv, nonce, &mut record);
                stored.extend_from_slice(&iv);
                stored.extend_from_slice(&record);
            }
        }
        let at = self.at(slab);
        self.file.write_at(at, &stored)
    }

    fn at(&self, block: u32) -> u64 {
        u64::from(block) * u64::from(self.bs)
    }

    /// The u32 at `offset`; 0 past the end of the file.
    fn u32(&mut self, offset: u64) -> io::Result<u32> {
        if offset + 4 > self.file.len()? {
            return Ok(0);
        }
        let mut bytes = [0; 4];
        self.file.read_at(offset, &mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn put32(&mut self, offset: u64, value: u32) -> io::Result<()> {
        self.file.write_at(offset, &value.to_le_bytes())
    }

    fn u8(&mut self, offset: u64) -> io::Result<u8> {
        let mut byte = [0];
        self.file.read_at(offset, &mut byte)?;
        Ok(byte[0])
    }

    fn put8(&mut self, offset: u64, value: u8) -> io::Result<()> {
        self.file.write_at(offset, &[value])
    }

    /// The end of the allocated space (header 0x00).
    fn end(&mut self) -> io::Result<u32> {
        let mut bytes = [0; 4];
        self.file.read_at(0, &mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn set_end(&mut self, end: u32) -> io::Result<()> {
        self.file.write_at(0, &end.to_be_bytes())
    }

    /// Counts an allocation in the header (0x0D), unless in a transaction.
    pub(crate) fn count_allocation(&mut self) -> io::Result<()> {
        if !self.counting {
            return Ok(());
        }
        let count = self.u8(0x0D)?;
        self.put8(0x0D, count.saturating_add(1))
    }

    /// The fragments: (size, first block).
    fn fragments(&mut self) -> io::Result<Vec<(u32, u32)>> {
        let mut count = [0; 2];
        self.file.read_at(8, &mut count)?;
        let mut fragments = Vec::new();
        for i in 0..u64::from(u16::from_le_bytes(count)) {
            fragments.push((
                self.u32(FRAGMENTS + 8 * i)?,
                self.u32(FRAGMENTS + 8 * i + 4)?,
            ));
        }
        Ok(fragments)
    }

    fn set_fragments(&mut self, fragments: &[(u32, u32)]) -> Result<(), Error> {
        if fragments.len() > MAX_FRAGMENTS {
            return Err(Error::Corrupt("memo file fragment list full"));
        }
        let old = self.fragments()?.len();
        self.file
            .write_at(8, &(fragments.len() as u16).to_le_bytes())?;
        for (i, &(size, block)) in fragments.iter().enumerate() {
            self.put32(FRAGMENTS + 8 * i as u64, size)?;
            self.put32(FRAGMENTS + 8 * i as u64 + 4, block)?;
        }
        // The entries left behind are zeroed.
        for i in fragments.len()..old {
            self.file.write_at(FRAGMENTS + 8 * i as u64, &[0; 8])?;
        }
        Ok(())
    }

    /// The size class of a run of `n` blocks.
    fn size_class(&self, n: u32) -> usize {
        let mut i = 0;
        while i + 1 < CLASS_COUNT && self.class_min[i + 1] <= n {
            i += 1;
        }
        i
    }

    /// The largest run of class `i`.
    fn class_max(&self, i: usize) -> u32 {
        match self.class_min.get(i + 1) {
            Some(&next) => next - 1,
            None => u32::MAX,
        }
    }

    /// Puts the free run of `size` blocks at `block` at the head of its
    /// class's list.
    fn push_run(&mut self, block: u32, size: u32) -> io::Result<()> {
        let head = class_head(self.size_class(size));
        let next = self.u32(head)?;
        self.write_run(block, size, next)?;
        self.put32(head, block)
    }

    /// The size and the next run of the free run at `block`: in an AES
    /// file behind an IV, which is zero for a run written in the clear
    /// and else enciphers them.
    fn run(&mut self, block: u32) -> io::Result<(u32, u32)> {
        let at = self.at(block);
        let prefix = self.records.prefix() as usize;
        let mut stored = vec![0; prefix + 8];
        self.file.read_at(at, &mut stored)?;
        let mut header: [u8; 8] = stored[prefix..].try_into().unwrap();
        if let Some((key, nonce)) = self.records.key() {
            let iv: [u8; 8] = stored[..8].try_into().unwrap();
            if iv != [0; 8] {
                crate::crypto::aes_ctr_xor(key, &iv, nonce, &mut header);
            }
        }
        let word = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
        Ok((word(0), word(4)))
    }

    /// Writes the header of the free run of `size` blocks at `block`, `next`
    /// the run after it on its list. In a file that encrypts, the header is
    /// enciphered under an IV of its own, and the rest of the block is left
    /// as it is.
    fn write_run(&mut self, block: u32, size: u32, next: u32) -> io::Result<()> {
        let mut header = [0; 8];
        header[..4].copy_from_slice(&size.to_le_bytes());
        header[4..].copy_from_slice(&next.to_le_bytes());
        let mut stored = Vec::with_capacity(16);
        match &self.records {
            SlabRecords::Plain => {}
            SlabRecords::Clear(_) => stored.extend_from_slice(&[0; 8]),
            SlabRecords::Enciphered {
                key,
                nonce,
                counter,
            } => {
                let iv = counter.fetch_add(1, Ordering::Relaxed).to_le_bytes();
                crate::crypto::aes_ctr_xor(key, &iv, nonce, &mut header);
                stored.extend_from_slice(&iv);
            }
        }
        stored.extend_from_slice(&header);
        let at = self.at(block);
        self.file.write_at(at, &stored)
    }

    /// Sets where `link` leads: to the run `next`.
    fn set_link(&mut self, link: Link, next: u32) -> io::Result<()> {
        match link {
            Link::Head(at) => self.put32(at, next),
            Link::Run(block) => {
                let (size, _) = self.run(block)?;
                self.write_run(block, size, next)
            }
        }
    }

    /// Takes the run at the head of class `i`'s list: its block and size.
    fn pop_run(&mut self, i: usize) -> io::Result<(u32, u32)> {
        let head = class_head(i);
        let block = self.u32(head)?;
        let (size, next) = self.run(block)?;
        self.put32(head, next)?;
        Ok((block, size))
    }

    /// Lists a free run of `size` blocks at `block`: the largest run the
    /// classes have from its front on a class list, over and over while 33
    /// blocks or more are left; the rest as a fragment, before the first
    /// fragment that starts after it (or first).
    fn carve(&mut self, mut block: u32, mut size: u32) -> Result<(), Error> {
        let mut fragments = self.fragments()?;
        while size >= self.class_min[25] {
            let i = self.size_class(size);
            let top = self.class_max(i);
            let run = if top <= size {
                top
            } else {
                self.class_max(i - 1)
            };
            self.push_run(block, run)?;
            block += run;
            size -= run;
        }
        if size > 0 {
            let at = fragments.iter().position(|&(_, b)| b > block).unwrap_or(0);
            fragments.insert(at, (size, block));
            self.set_fragments(&fragments)?;
        }
        Ok(())
    }

    /// Merges a free run into a fragment next to it, as the original engine
    /// finds one: it looks only at the first fragment that starts after the
    /// run and the one before that (the list is not sorted, but it is
    /// searched as if it were); a run that fills the gap between both joins
    /// them. The index of the fragment it went into.
    fn merge_fragment(fragments: &mut Vec<(u32, u32)>, block: u32, size: u32) -> Option<usize> {
        let i = fragments.iter().position(|&(_, b)| b > block)?;
        if i > 0 && fragments[i - 1].1 + fragments[i - 1].0 == block {
            let (mut before_size, before_block) = fragments[i - 1];
            before_size += size;
            if before_block + before_size == fragments[i].1 {
                before_size += fragments[i].0;
                fragments.remove(i);
            }
            fragments[i - 1] = (before_size, before_block);
            return Some(i - 1);
        }
        if block + size == fragments[i].1 {
            fragments[i] = (fragments[i].0 + size, block);
            return Some(i);
        }
        None
    }

    /// Frees a run: merged with a fragment next to it if there is one (a
    /// fragment that grows past a slab chunk leaves the list), and what is
    /// left carved onto the lists.
    fn release(&mut self, block: u32, size: u32) -> Result<(), Error> {
        let mut fragments = self.fragments()?;
        let (mut block, mut size) = (block, size);
        if let Some(k) = Allocator::merge_fragment(&mut fragments, block, size) {
            (size, block) = fragments[k];
            if size <= MAX_CHUNK {
                return self.set_fragments(&fragments);
            }
            fragments.remove(k);
        }
        self.set_fragments(&fragments)?;
        self.carve(block, size)
    }

    /// Extends the allocated space to the end of the next slab position, for
    /// a new slab: the file is set to end with that slab, and the space
    /// before it carved.
    fn grow(&mut self) -> Result<(), Error> {
        let start = self.end()?;
        let slab = start.div_ceil(SLAB_ALIGN) * SLAB_ALIGN;
        let length = self.at(slab + SLAB_ALIGN);
        self.file.set_len(length)?;
        if slab > start {
            self.carve(start, slab - start)?;
        }
        self.push_run(slab, SLAB_SPAN)?;
        Ok(self.set_end(slab + SLAB_SPAN)?)
    }

    /// A run for `n` (more than 32) blocks, at the largest size of their
    /// class: the head of the first list from that class up that has one,
    /// split; else added at the end of the allocated space.
    fn alloc_run(&mut self, n: u32) -> Result<u32, Error> {
        let class = self.size_class(n);
        let size = self.class_max(class);
        for i in class..CLASS_COUNT {
            if self.u32(class_head(i))? != 0 {
                let (block, run) = self.pop_run(i)?;
                if run > size {
                    self.release(block + size, run - size)?;
                }
                return Ok(block);
            }
        }
        let block = self.end()?;
        self.set_end(block + size)?;
        Ok(block)
    }

    /// The 128-aligned span of 129 blocks for a new slab: from the first free
    /// run that holds one (the lists from 129 blocks up, each in order), what
    /// is around it freed again; else from growing the file.
    fn alloc_slab_run(&mut self) -> Result<u32, Error> {
        loop {
            for i in self.size_class(SLAB_SPAN)..CLASS_COUNT {
                let mut previous = Link::Head(class_head(i));
                let mut block = self.u32(class_head(i))?;
                while block != 0 {
                    let (size, next) = self.run(block)?;
                    let slab = block.div_ceil(SLAB_ALIGN) * SLAB_ALIGN;
                    if slab + SLAB_SPAN <= block + size {
                        self.set_link(previous, next)?;
                        if slab > block {
                            self.release(block, slab - block)?;
                        }
                        if block + size > slab + SLAB_SPAN {
                            self.release(slab + SLAB_SPAN, block + size - slab - SLAB_SPAN)?;
                        }
                        return Ok(slab);
                    }
                    previous = Link::Run(block);
                    block = next;
                }
            }
            self.grow()?;
        }
    }

    /// Frees an extent of `n` blocks at `block`: at the end of the allocated
    /// space, the space ends there instead.
    fn free_run(&mut self, block: u32, n: u32) -> Result<(), Error> {
        let size = self.class_max(self.size_class(n));
        if block + size == self.end()? {
            return Ok(self.set_end(block)?);
        }
        self.release(block, size)
    }

    /// A chunk of `c` blocks: the lowest free one of the slab at the head of
    /// the chunk size's list, from a new slab if the list is empty. A slab
    /// that fills up leaves its list.
    fn alloc_chunk(&mut self, c: u32) -> Result<u32, Error> {
        let head = slab_head(c);
        let mut slab = self.u32(head)?;
        let mut record;
        if slab == 0 {
            slab = self.alloc_slab_run()?;
            record = [0; META_BYTES as usize];
            record[..4].copy_from_slice(&[
                MARKER,
                ((SLAB_ALIGN - self.meta_blocks) / c) as u8,
                0,
                c as u8,
            ]);
            let next = self.u32(head)?;
            record[4..8].copy_from_slice(&next.to_le_bytes());
            self.put32(head, slab)?;
        } else {
            record = self.slab_record(slab)?;
        }
        let (slots, used) = (record[1], record[2]);
        let bitmap = &mut record[8..];
        let slot = (0..usize::from(slots))
            .find(|&i| bitmap[i / 8] >> (i % 8) & 1 == 0)
            .ok_or(Error::Corrupt("memo file slab without a free chunk"))?;
        bitmap[slot / 8] |= 1 << (slot % 8);
        record[2] = used + 1;
        if used + 1 == slots {
            self.put32(head, u32::from_le_bytes(record[4..8].try_into().unwrap()))?;
            record[4..8].fill(0);
        }
        self.set_slab_record(slab, &record)?;
        Ok(slab + self.meta_blocks + slot as u32 * c)
    }

    /// Frees the chunk at `block`. A slab that was full goes back on its
    /// list; one whose last chunk frees is freed itself if it heads its list,
    /// and else keeps the chunk (as the original engine does); the last
    /// chunk's bit stays set.
    fn free_chunk(&mut self, block: u32) -> Result<(), Error> {
        let slab = block - block % SLAB_ALIGN;
        let mut record = self.slab_record(slab)?;
        let (slots, used, c) = (record[1], record[2], u32::from(record[3]));
        if c == 0 {
            return Err(Error::Corrupt("memo file chunk outside a slab"));
        }
        let slot = ((block - slab - self.meta_blocks) / c) as usize;
        let head = slab_head(c);
        if used == 1 && self.u32(head)? != slab {
            return Ok(());
        }
        if used > 1 {
            record[8 + slot / 8] &= !(1 << (slot % 8));
        }
        record[2] = used.saturating_sub(1);
        if used == slots {
            let first = self.u32(head)?;
            record[4..8].copy_from_slice(&first.to_le_bytes());
            self.put32(head, slab)?;
        }
        self.set_slab_record(slab, &record)?;
        if used == 1 {
            self.put32(head, u32::from_le_bytes(record[4..8].try_into().unwrap()))?;
            self.push_run(slab, SLAB_SPAN)?;
        }
        Ok(())
    }

    /// Forgets the free space: the fragments and the lists of free runs and
    /// of slabs with free chunks are emptied, and what they held is left
    /// where it is, taken. The end of the allocated space and the count of
    /// allocations stay.
    pub(crate) fn forget_free_space(&mut self) -> io::Result<()> {
        self.file.write_at(8, &[0; 2])?;
        self.file.write_at(FRAGMENTS, &[0; 0x3F0])?;
        for i in 0..CLASS_COUNT {
            self.put32(class_head(i), 0)?;
        }
        for chunk in 1..=MAX_CHUNK {
            self.put32(slab_head(chunk), 0)?;
        }
        Ok(())
    }

    /// The blocks a value of `length` bytes takes: at least one.
    pub(crate) fn blocks(&self, length: u32) -> u32 {
        length.div_ceil(self.bs).max(1)
    }

    /// Whether the file is one `AdsZapTable` emptied: only the 1,024 bytes of
    /// header before the size classes, which the original engine then fills
    /// without them, putting each value in a run of blocks of its own
    /// (`fixtures/sql/spt`).
    fn zapped(&self) -> bool {
        self.class_min[0] == 0
    }

    /// Space for `n` blocks in a zapped file: the first fragment that holds
    /// them, else the end of the allocated space.
    fn alloc_zapped(&mut self, n: u32) -> Result<u32, Error> {
        let mut fragments = self.fragments()?;
        if let Some(i) = fragments.iter().position(|&(size, _)| size >= n) {
            let (size, block) = fragments[i];
            match size == n {
                true => drop(fragments.remove(i)),
                false => fragments[i] = (size - n, block + n),
            }
            self.set_fragments(&fragments)?;
            return Ok(block);
        }
        let block = self.end()?;
        self.set_end(block + n)?;
        Ok(block)
    }

    /// Frees `n` blocks at `block` in a zapped file: the space ends there if
    /// they were its last, else they are a fragment, merged with one next to
    /// it.
    fn free_zapped(&mut self, block: u32, n: u32) -> Result<(), Error> {
        if block + n == self.end()? {
            return Ok(self.set_end(block)?);
        }
        let mut fragments = self.fragments()?;
        if Allocator::merge_fragment(&mut fragments, block, n).is_none() {
            let at = fragments.iter().position(|&(_, b)| b > block).unwrap_or(0);
            fragments.insert(at, (n, block));
        }
        self.set_fragments(&fragments)
    }

    /// Space for a value of `length` bytes: its first block.
    pub(crate) fn alloc(&mut self, length: u32) -> Result<u32, Error> {
        let n = self.blocks(length);
        self.count_allocation()?;
        if self.zapped() {
            return self.alloc_zapped(n);
        }
        match n <= MAX_CHUNK {
            true => self.alloc_chunk(n),
            false => self.alloc_run(n),
        }
    }

    /// Frees the value of `length` bytes at `block`.
    pub(crate) fn free(&mut self, block: u32, length: u32) -> Result<(), Error> {
        let n = self.blocks(length);
        if self.zapped() {
            return self.free_zapped(block, n);
        }
        match n <= MAX_CHUNK {
            true => self.free_chunk(block),
            false => self.free_run(block, n),
        }
    }

    /// Takes back the value of `length` bytes at `block`, freed when its
    /// record was deleted, as the original engine does when it recalls the
    /// record: a chunk is marked in use again in its slab, unless the slab
    /// went back to the free space whole; a larger run stays free.
    pub(crate) fn reclaim(&mut self, block: u32, length: u32) -> Result<(), Error> {
        if self.blocks(length) > MAX_CHUNK {
            return Ok(());
        }
        let slab = block - block % SLAB_ALIGN;
        let mut record = self.slab_record(slab)?;
        let (used, c) = (record[2], u32::from(record[3]));
        if record[0] != MARKER || c == 0 || used == 0 {
            return Ok(());
        }
        let slot = ((block - slab - self.meta_blocks) / c) as usize;
        if record[8 + slot / 8] & (1 << (slot % 8)) != 0 {
            return Ok(());
        }
        record[8 + slot / 8] |= 1 << (slot % 8);
        record[2] = used + 1;
        Ok(self.set_slab_record(slab, &record)?)
    }

    /// Writes `data` at the start of block `block`.
    pub(crate) fn write(&mut self, block: u32, data: &[u8]) -> io::Result<()> {
        let at = self.at(block);
        self.file.write_at(at, data)
    }
}
