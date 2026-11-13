//! Writing the memo files of DBF tables, as the original engine writes
//! them: Clipper's `.dbt` and FoxPro's `.fpt` (see [`crate::dbf`] for
//! reading them).
//!
//! A `.dbt` has 512-byte blocks and no free list: its header holds only the
//! next free block. A value is followed by a terminator (0x1A after text,
//! 0x00 after an ADS VarChar's remainder). One that is rewritten stays where
//! it is if it fits ([`dbt_fits`]); else it is appended: a 0x1A is written at
//! the end of the file, the value at the next block boundary, and the old
//! blocks are abandoned.
//!
//! An `.fpt` value starts with a block header (its type, 1, and length, both
//! big-endian; a General field's type is 2 and a Picture's 0, as
//! `AdsConvertTable` writes them), but for an ADS VarChar's remainder, which
//! is raw. The header
//! keeps the next free block (big-endian), the block size, and a list of free
//! runs (up to 82, by block). A value that needs as many blocks as the old one
//! is written over it; one that needs fewer keeps its start and returns its
//! tail; one that needs more returns all its blocks and is allocated anew.
//! Allocation takes the first run that is large enough, else the blocks at
//! the next free block.

use crate::dbf::Layout;
use crate::field::{Field, FieldType};
use crate::memo::{MemoFile, MemoKind};
use crate::table::Error;
use std::sync::atomic::Ordering;

/// The tags that start a Binary and an Image value in a Clipper or FoxPro
/// table's memo file, before their length.
const BINARY_TAG: &[u8; 12] = b"@R_BLOB@\x00\x1a\x0b\x0c";
const IMAGE_TAG: &[u8; 12] = b"@SIXPIC@\x00\x1a\x0b\x0c";

/// The `.dbt` block size.
const DBT_BLOCK: u64 = 512;

/// The most free runs an `.fpt` header lists.
const FPT_RUNS: usize = 82;

/// Where an `.fpt` header keeps its free list: the number of runs, then
/// each run's u16 length in blocks and u32 first block.
const FPT_RUN_COUNT: u64 = 0x12;
const FPT_RUN_LIST: u64 = 0x14;

/// The signature an `.fpt` header gets when blocks are first allocated or
/// freed in it.
const FPT_SIGNATURE: &[u8; 8] = b"SIxMemo\0";

/// Stores the value of memo field `field` (as the memo file keeps it: the
/// text's bytes, UTF-16LE for NMemo, a binary value's bytes; `None` for none)
/// in `memo`, and its reference in `record`. No value clears the reference
/// (its blocks are abandoned).
pub(crate) fn store(
    layout: &Layout,
    field: &Field,
    value: Option<&[u8]>,
    record: &mut [u8],
    memo: &mut MemoFile,
) -> Result<(), Error> {
    let old = layout.memo_ref(field, record).block;
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        write_ref(layout, field, record, 0);
        layout.set_not_null(record);
        return Ok(());
    };
    let tagged =
        !layout.vfp_table() && matches!(field.field_type, FieldType::Binary | FieldType::Image);
    let mut data = Vec::with_capacity(value.len() + 16);
    if tagged {
        data.extend_from_slice(match field.field_type {
            FieldType::Image => IMAGE_TAG,
            _ => BINARY_TAG,
        });
        data.extend_from_slice(&(value.len() as u32).to_le_bytes());
    }
    data.extend_from_slice(value);
    let block = match memo.kind() {
        MemoKind::Dbt => {
            // Text ends with 0x1A; after binary data the original leaves a
            // byte it does not set.
            data.push(0x1A);
            let old_length = match (old, memo.rc4_write().is_some()) {
                (0, _) => None,
                // The old value's end is in an enciphered stream.
                (old, true) => Some(memo.read_dbf(old, tagged)?.len()),
                (old, false) => Some(dbt_length(memo, old, tagged)?),
            };
            // The value and its 0x1A are one stream at the record's number;
            // a Binary or Image value is enciphered after its tag, and its
            // 0x1A stays clear (`fixtures/aes/rc4tp/y13`).
            if let Some((key, item)) = memo.rc4_write() {
                match tagged {
                    true => {
                        let end = data.len() - 1;
                        crate::crypto::rc4_crypt(&key, item, &mut data[16..end]);
                    }
                    false => crate::crypto::rc4_crypt(&key, item, &mut data),
                }
            }
            let block = dbt_write(memo, old, old_length, &data)?;
            // The original engine ends binary data with a byte it does not
            // set.
            if tagged {
                let end = u64::from(block) * DBT_BLOCK + data.len() as u64 - 1;
                crate::undefined::note(memo.path(), end);
            }
            block
        }
        _ => {
            // An AES table's value is behind an 8-byte IV, which takes room
            // in its blocks.
            let overhead = memo.value_overhead() as usize;
            let old_blocks = match old {
                0 => None,
                old => {
                    let length = fpt_length(memo, old)?;
                    Some(fpt_blocks(memo, length + 8 + overhead))
                }
            };
            let block_type: u32 = match field.field_type {
                FieldType::Other(code) if code == u16::from(b'G') => 2,
                FieldType::Other(code) if code == u16::from(b'P') => 0,
                _ => 1,
            };
            let mut stored = Vec::with_capacity(data.len() + 8);
            stored.extend_from_slice(&block_type.to_be_bytes());
            stored.extend_from_slice(&(data.len() as u32).to_be_bytes());
            // The block header stays clear, and so does a SIx tag.
            if let Some((key, item)) = memo.rc4_write() {
                let tag = if tagged { 16 } else { 0 };
                crate::crypto::rc4_crypt(&key, item, &mut data[tag..]);
            }
            if overhead != 0 {
                match memo.aes_cipher() {
                    // The value is enciphered to the end of its blocks, the
                    // bytes after it what the original's buffer held
                    // (`fixtures/aes/rc4tp`, `v1`).
                    Some((key, nonce)) => {
                        let room = fpt_blocks(memo, 16 + data.len()) as usize
                            * memo.block_size() as usize
                            - 16;
                        data.resize(room, 0);
                        let iv = memo
                            .aes_counter()
                            .fetch_add(room.div_ceil(16) as u64, Ordering::Relaxed)
                            .to_le_bytes();
                        crate::crypto::aes_ctr_xor(&key, &iv, &nonce, &mut data);
                        stored.extend_from_slice(&iv);
                    }
                    None => stored.extend_from_slice(&[0; 8]),
                }
            }
            stored.extend_from_slice(&data);
            fpt_write(memo, old, old_blocks, &stored)?
        }
    };
    write_ref(layout, field, record, block);
    layout.set_not_null(record);
    Ok(())
}

/// Stores the remainder of an ADS VarChar's text, the part after the
/// field's size − 6 inline bytes, in `memo`, rewriting the one at block
/// `old` (0 for none) where it can: its first block.
pub(crate) fn store_remainder(
    memo: &mut MemoFile,
    old: u32,
    remainder: &[u8],
) -> Result<u32, Error> {
    match memo.kind() {
        MemoKind::Dbt => {
            let mut data = remainder.to_vec();
            data.push(0);
            let old_length = match old {
                0 => None,
                old => Some(dbt_length(memo, old, false)?),
            };
            dbt_write(memo, old, old_length, &data)
        }
        // An old remainder counts as one block, whatever its length.
        _ => fpt_write(memo, old, (old != 0).then_some(1), remainder),
    }
}

/// Writes the reference to block `block` (0 for none) into `field` in
/// `record`: ten digits, right-aligned (spaces for none), or a u32 in a VFP
/// table.
fn write_ref(layout: &Layout, field: &Field, record: &mut [u8], block: u32) {
    let start = usize::from(field.offset);
    let slot = &mut record[start..start + usize::from(field.size)];
    if layout.vfp_table() {
        slot.fill(0);
        slot[..4].copy_from_slice(&block.to_le_bytes());
        return;
    }
    match block {
        0 => slot.fill(b' '),
        block => {
            let text = format!("{block:>width$}", width = slot.len());
            slot.copy_from_slice(&text.as_bytes()[..slot.len()]);
        }
    }
}

/// The length of the `.dbt` value at block `block`: up to the first 0x1A,
/// or for a tagged binary value its tag and data. An ADS VarChar's remainder
/// ends with 0x00, so its length runs on to the 0x1A that the next value
/// appended put after it; the last value of the file has none, and the
/// original engine fails ([`Error::CorruptMemo`]).
fn dbt_length(memo: &mut MemoFile, block: u32, tagged: bool) -> Result<usize, Error> {
    if tagged {
        let tag = memo.read_raw(block, 16)?;
        return Ok(16 + u32::from_le_bytes(tag[12..16].try_into().unwrap()) as usize);
    }
    let file = memo.source();
    let size = file.len()?;
    let start = u64::from(block) * DBT_BLOCK;
    let mut at = start;
    let mut chunk = [0; 512];
    while at < size {
        let n = (size - at).min(chunk.len() as u64) as usize;
        file.read_at(at, &mut chunk[..n])?;
        if let Some(end) = chunk[..n].iter().position(|&b| b == 0x1A) {
            return Ok((at - start) as usize + end);
        }
        at += n as u64;
    }
    Err(Error::CorruptMemo)
}

/// Whether a `.dbt` value of `n` bytes (its terminator included) is written
/// over an old one of `old` bytes: if it fits one block, or does not reach
/// a block further than the old one.
fn dbt_fits(n: usize, old: usize) -> bool {
    let block = DBT_BLOCK as usize;
    n <= block || n / block * block <= old / block * block
}

/// Writes `data` (with its terminator) to the `.dbt`: over the old value at
/// block `old` if it fits, else at the end. Returns its block.
fn dbt_write(
    memo: &mut MemoFile,
    old: u32,
    old_length: Option<usize>,
    data: &[u8],
) -> Result<u32, Error> {
    let moving = memo.moving();
    let file = memo.source();
    if !moving && old_length.is_some_and(|length| dbt_fits(data.len(), length)) {
        file.write_at(u64::from(old) * DBT_BLOCK, data)?;
        return Ok(old);
    }
    let end = file.len()?;
    file.write_at(end, &[0x1A])?;
    let block = (end + 1).div_ceil(DBT_BLOCK);
    let at = block * DBT_BLOCK;
    file.write_at(at, data)?;
    let next = (at + data.len() as u64).div_ceil(DBT_BLOCK);
    file.write_at(0, &(next as u32).to_le_bytes())?;
    Ok(block as u32)
}

/// The length of the `.fpt` value at block `block`, from its block header.
fn fpt_length(memo: &mut MemoFile, block: u32) -> Result<usize, Error> {
    let header = memo.read_raw(block, 8)?;
    Ok(u32::from_be_bytes(header[4..8].try_into().unwrap()) as usize)
}

/// How many blocks `bytes` bytes take in the `.fpt`.
fn fpt_blocks(memo: &MemoFile, bytes: usize) -> u32 {
    (bytes as u32).div_ceil(memo.block_size()).max(1)
}

/// Writes `stored` (a value with its block header, or a raw remainder) to
/// the `.fpt`, over the value at block `old` of `old_blocks` blocks where it
/// can. Returns its block.
fn fpt_write(
    memo: &mut MemoFile,
    old: u32,
    old_blocks: Option<u32>,
    stored: &[u8],
) -> Result<u32, Error> {
    let blocks = fpt_blocks(memo, stored.len());
    let moving = memo.moving();
    let block = match old_blocks {
        // A value another handle's record image still points at goes to new
        // blocks first, and the old ones are given back after
        // (`fixtures/write/memo_other_handle`).
        Some(old_blocks) if moving => {
            let mut header = FptHeader::read(memo)?;
            let block = header.alloc(blocks);
            header.free(old, old_blocks);
            header.write(memo)?;
            block
        }
        Some(old_blocks) if blocks == old_blocks => old,
        Some(old_blocks) if blocks < old_blocks => {
            let mut header = FptHeader::read(memo)?;
            header.free(old + blocks, old_blocks - blocks);
            header.write(memo)?;
            old
        }
        old_blocks => {
            let mut header = FptHeader::read(memo)?;
            if let Some(old_blocks) = old_blocks {
                header.free(old, old_blocks);
            }
            let block = header.alloc(blocks);
            header.write(memo)?;
            block
        }
    };
    let block_size = u64::from(memo.block_size());
    memo.source()
        .write_at(u64::from(block) * block_size, stored)?;
    Ok(block)
}

/// What an `.fpt` header says about its blocks.
struct FptHeader {
    next: u32,
    /// The free runs: (first block, length in blocks), by block.
    runs: Vec<(u32, u16)>,
    /// How many runs the header listed when it was read, which are zeroed
    /// when the list is written shorter.
    listed: usize,
    signed: bool,
}

impl FptHeader {
    fn read(memo: &mut MemoFile) -> Result<FptHeader, Error> {
        let mut head = vec![0; 512];
        memo.source().read_at(0, &mut head)?;
        let next = u32::from_be_bytes(head[0..4].try_into().unwrap());
        let signed = head[8..16] == FPT_SIGNATURE[..];
        let at = FPT_RUN_COUNT as usize;
        let count = match signed {
            true => usize::from(u16::from_le_bytes([head[at], head[at + 1]])).min(FPT_RUNS),
            false => 0,
        };
        let runs = (0..count)
            .map(|i| {
                let at = FPT_RUN_LIST as usize + 6 * i;
                let length = u16::from_le_bytes([head[at], head[at + 1]]);
                let block = u32::from_le_bytes(head[at + 2..at + 6].try_into().unwrap());
                (block, length)
            })
            .collect();
        Ok(FptHeader {
            next,
            runs,
            listed: count,
            signed,
        })
    }

    /// Takes `blocks` blocks: from the start of the first run that has
    /// enough, else at the next free block.
    fn alloc(&mut self, blocks: u32) -> u32 {
        if let Some(i) = self
            .runs
            .iter()
            .position(|&(_, length)| u32::from(length) >= blocks)
        {
            let (block, length) = self.runs[i];
            match u32::from(length) == blocks {
                true => {
                    self.runs.remove(i);
                }
                false => self.runs[i] = (block + blocks, (u32::from(length) - blocks) as u16),
            }
            return block;
        }
        let block = self.next;
        self.next += blocks;
        block
    }

    /// Returns `blocks` blocks from `block` on: blocks that end at the next
    /// free block move it back; a run after every listed one is appended
    /// unmerged; any other is inserted by block and merged with the runs it
    /// adjoins. A full list drops the run (or, inserting in the middle, its
    /// last one).
    fn free(&mut self, block: u32, blocks: u32) {
        if block + blocks == self.next {
            self.next = block;
            return;
        }
        let length = blocks.min(0xFFFF) as u16;
        let at = self
            .runs
            .iter()
            .position(|&(b, _)| b > block)
            .unwrap_or(self.runs.len());
        if at == self.runs.len() {
            if self.runs.len() < FPT_RUNS {
                self.runs.push((block, length));
            }
            return;
        }
        let before = at.checked_sub(1).filter(|&i| {
            let (b, l) = self.runs[i];
            b + u32::from(l) == block
        });
        let after = (self.runs[at].0 == block + blocks).then_some(at);
        match (before, after) {
            (Some(i), Some(j)) => {
                let merged = self.runs[i]
                    .1
                    .wrapping_add(length)
                    .wrapping_add(self.runs[j].1);
                self.runs[i].1 = merged;
                self.runs.remove(j);
            }
            (Some(i), None) => self.runs[i].1 = self.runs[i].1.wrapping_add(length),
            (None, Some(j)) => self.runs[j] = (block, length.wrapping_add(self.runs[j].1)),
            (None, None) => {
                if self.runs.len() == FPT_RUNS {
                    self.runs.pop();
                }
                self.runs.insert(at, (block, length));
            }
        }
    }

    fn write(&self, memo: &mut MemoFile) -> Result<(), Error> {
        let file = memo.source();
        let mut head = vec![0; 512];
        file.read_at(0, &mut head)?;
        if !self.signed {
            head[8..16].copy_from_slice(FPT_SIGNATURE);
            head[0x10..].fill(0);
        }
        head[0..4].copy_from_slice(&self.next.to_be_bytes());
        let at = FPT_RUN_COUNT as usize;
        head[at..at + 2].copy_from_slice(&(self.runs.len() as u16).to_le_bytes());
        for i in 0..self.listed.max(self.runs.len()) {
            let at = FPT_RUN_LIST as usize + 6 * i;
            let (block, length) = self.runs.get(i).copied().unwrap_or((0, 0));
            head[at..at + 2].copy_from_slice(&length.to_le_bytes());
            head[at + 2..at + 6].copy_from_slice(&block.to_le_bytes());
        }
        Ok(file.write_at(0, &head)?)
    }
}
