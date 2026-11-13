//! The table image of an online backup: a copy of a table made as the
//! original's server makes it (`fixtures/backup/`), and the changes a restore
//! makes to such an image to turn it into a table again.
//!
//! The copy is made record by record rather than by copying the files, so
//! that it holds a consistent table whatever the source's memo file looked
//! like: memo values are written afresh, one value after the other.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::dbf;
use crate::field::Field;
use crate::memo::{MemoFile, MemoKind};
use crate::table::{Error, Format, RecordStatus, TableFile};
use crate::{adt, create};

/// What a copy into an image is asked to do.
#[derive(Debug, Clone, Default)]
pub struct Copy {
    /// Keep the ADT records that are deleted (a differential image's first
    /// copy does); a DBF table's are always kept.
    pub keep_deleted: bool,
    /// The password an RC4-encrypted ADT table's records and field
    /// descriptors are enciphered with.
    pub password: Option<Vec<u8>>,
    /// Records as a transaction not ended yet has them in the file, by slot:
    /// the bytes they had before it (the image holds the committed table),
    /// or none for a record it appended, which the image leaves out.
    pub overrides: std::collections::BTreeMap<u32, Option<Vec<u8>>>,
    /// The flavour a DBF image is written as, when it is not the source's:
    /// a Visual FoxPro table becomes a FoxPro or Clipper one and a FoxPro
    /// one a Visual FoxPro one (a free-table backup opens a table as the
    /// type its call asks for).
    pub flavour: Option<dbf::Flavour>,
}

/// Writes the image of the table at `source`, of format `format`, to
/// `image`, and its memo file next to it (replacing files that are there).
///
/// An ADT image has no record that is deleted (unless `copy.keep_deleted`),
/// no free list, and the header of a free table; a DBF image keeps every
/// record and loses the structural index bit. Memo values are written again
/// in record order, so that the blocks of the memo file are renumbered. An
/// encrypted table's records stay enciphered as they are. The table is read
/// through `source_file`, which the caller opened (with the password of an
/// encrypted table's descriptors where it needs one).
pub fn write_image(
    source_file: &mut TableFile,
    source: &Path,
    image: &Path,
    copy: &Copy,
) -> Result<(), Error> {
    match source_file.structure.header.format {
        Format::Adt => write_adt(source_file, source, image, copy),
        Format::Dbf(_) => write_dbf(source_file, source, image, copy),
    }
}

fn memo_fields(file: &TableFile) -> Vec<Field> {
    file.structure
        .fields
        .iter()
        .filter(|f| f.is_memo())
        .cloned()
        .collect()
}

/// The key an RC4 table's descriptors and records are enciphered with.
fn rc4_key(password: &[u8]) -> &[u8] {
    &password[..password.len().min(crate::crypto::KEY_MAX)]
}

/// Enciphers or deciphers the field descriptors of the ADT table at `table`,
/// whose header says how many fields it has and where its records start, as a
/// dictionary of RC4 tables keeps them: one stream, at the item that is `0x45`
/// for each field. The flag that says they are enciphered follows.
pub fn rc4_descriptors(table: &Path, password: &[u8], on: bool) -> Result<(), Error> {
    let mut file = OpenOptions::new().read(true).write(true).open(table)?;
    let mut head = [0; adt::HEADER_SIZE];
    file.read_exact(&mut head)?;
    let flags = u32::from_le_bytes(head[0x58..0x5C].try_into().unwrap());
    let fields = u32::from(u16::from_le_bytes(head[0x166..0x168].try_into().unwrap()));
    let extension = u64::from(u32::from_le_bytes(head[0x170..0x174].try_into().unwrap()));
    let data = u64::from(u32::from_le_bytes(head[0x20..0x24].try_into().unwrap()));
    let start = adt::HEADER_SIZE as u64 + extension;
    let mut region = vec![0; (data - start) as usize];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut region)?;
    crate::crypto::rc4_crypt(rc4_key(password), 0x45 * fields, &mut region);
    file.seek(SeekFrom::Start(start))?;
    file.write_all(&region)?;
    let flag = crate::table::flags::ENCRYPTED_DESCRIPTORS;
    let flags = if on { flags | flag } else { flags & !flag };
    file.seek(SeekFrom::Start(0x58))?;
    file.write_all(&flags.to_le_bytes())?;
    Ok(())
}

/// An encrypted ADT table's image: the records and memo values are read
/// deciphered and enciphered again for the places they move to, then the
/// descriptors are put in the clear.
fn write_adt_encrypted(
    source: &Path,
    image: &Path,
    password: &[u8],
    copy: &Copy,
) -> Result<(), Error> {
    fs::copy(source, image)?;
    let source_memo = MemoFile::path_for(source);
    let image_memo = MemoFile::path_for(image);
    let block_size = {
        let file = TableFile::open_deciphering(source, Format::Adt, false, Some(password))?;
        file.memo_block_size()
    };
    if let Some(block_size) = block_size {
        fs::write(&image_memo, create::adm_bytes(block_size as u16))?;
    } else if source_memo.exists() {
        fs::copy(&source_memo, &image_memo)?;
    }
    let mut gone = Vec::new();
    let (slots, record_length, data_offset) = {
        let mut from = TableFile::open_deciphering(source, Format::Adt, false, Some(password))?;
        let mut to = TableFile::open_deciphering(image, Format::Adt, true, Some(password))?;
        if let Some(memo) = to.memo() {
            memo.set_counting(false);
        }
        let header = from.structure.header.clone();
        let memo_fields = memo_fields(&from);
        let mut kept = 0;
        for slot in 1..=header.record_slots {
            let deleted = from.status(slot)? == RecordStatus::Deleted;
            if deleted && !copy.keep_deleted {
                continue;
            }
            kept += 1;
            if deleted {
                gone.push(kept);
            }
            let mut record = from.record(slot)?;
            let mut values = Vec::new();
            // A deleted record keeps its memo pointer as it is, and takes no
            // block (`fixtures/backup/s50_diff`).
            let fields = if deleted { &[][..] } else { &memo_fields[..] };
            for field in fields {
                let memo = from.memo().ok_or(Error::CorruptMemo)?;
                memo.decipher_record(slot);
                values.push(field.memo_data(&record, Some(memo))?.unwrap_or_default());
            }
            for (field, value) in fields.iter().zip(values) {
                let at = usize::from(field.offset);
                record[at..at + usize::from(field.size)].fill(0);
                let memo = to.memo().ok_or(Error::CorruptMemo)?;
                memo.decipher_record(kept);
                let value = (!value.is_empty()).then_some(&value[..]);
                field.store_memo(value, &mut record, memo)?;
            }
            to.write(kept, &record)?;
        }
        (kept, header.record_length, header.data_offset)
    };
    let mut out = OpenOptions::new().read(true).write(true).open(image)?;
    out.seek(SeekFrom::Start(0x18))?;
    out.write_all(&slots.to_le_bytes())?;
    out.seek(SeekFrom::Start(0x28))?;
    out.write_all(&[0; 4])?;
    out.seek(SeekFrom::Start(0x30))?;
    out.write_all(&[0; 4])?;
    out.set_len(u64::from(data_offset) + u64::from(slots) * u64::from(record_length))?;
    // The write above marks every record active.
    for slot in gone {
        let at = u64::from(data_offset) + u64::from(slot - 1) * u64::from(record_length);
        out.seek(SeekFrom::Start(at))?;
        out.write_all(&[adt::ENCRYPTED_DELETED])?;
    }
    out.seek(SeekFrom::Start(adt::offsets::BACKUP_STAMP))?;
    out.write_all(&[0; 4])?;
    drop(out);
    let flags_at = |image: &Path| -> Result<u32, Error> {
        let mut file = File::open(image)?;
        let mut bytes = [0; 4];
        file.seek(SeekFrom::Start(0x58))?;
        file.read_exact(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    };
    if flags_at(image)? & crate::table::flags::ENCRYPTED_DESCRIPTORS != 0 {
        rc4_descriptors(image, password, false)?;
    }
    let mut out = OpenOptions::new().read(true).write(true).open(image)?;
    out.seek(SeekFrom::Start(0x58))?;
    out.write_all(&(flags_at(image)? | 0x10).to_le_bytes())?;
    crate::dd::create::free_table(image)?;
    Ok(())
}

fn write_adt(file: &mut TableFile, source: &Path, image: &Path, copy: &Copy) -> Result<(), Error> {
    if let Some(password) = copy
        .password
        .as_deref()
        .filter(|_| file.structure.header.encrypted())
    {
        return write_adt_encrypted(source, image, password, copy);
    }
    let header = file.structure.header.clone();
    let mut head = vec![0; header.data_offset as usize];
    File::open(source)?.read_exact(&mut head)?;
    let memo_fields = memo_fields(file);
    let encrypted = header.encrypted();
    let mut kept = Vec::new();
    for slot in 1..=header.record_slots {
        let mut record = vec![0; header.record_length as usize];
        match copy.overrides.get(&slot) {
            Some(None) => continue,
            Some(Some(before)) => record.copy_from_slice(before),
            None => file.read_stored(slot, &mut record)?,
        }
        let deleted = matches!(file.status_of(&record), RecordStatus::Deleted);
        if deleted && !copy.keep_deleted {
            continue;
        }
        kept.push((record, deleted));
    }
    let mut memo = None;
    if !memo_fields.is_empty() && file.memo().is_some() {
        let block_size = file.memo_block_size().unwrap_or(256) as u16;
        let path = MemoFile::path_for(image);
        fs::write(&path, create::adm_bytes(block_size))?;
        let mut target = MemoFile::open_adm(&path, true)?;
        // The original's image does not count the allocations it makes.
        target.set_counting(false);
        memo = Some(target);
    }
    let mut out = Vec::with_capacity(head.len() + kept.len() * header.record_length as usize);
    out.extend_from_slice(&head);
    for (record, deleted) in &mut kept {
        // The free list's link in a deleted record is not kept.
        record[1..adt::RECORD_PREFIX].fill(0);
        // A deleted record keeps its memo pointer as it is, and takes no
        // block (`fixtures/backup/s50_diff`).
        if let Some(target) = memo.as_mut().filter(|_| !*deleted) {
            let source_memo = file.memo().ok_or(Error::CorruptMemo)?;
            for field in &memo_fields {
                let value = field
                    .memo_data(record, Some(&mut *source_memo))?
                    .unwrap_or_default();
                let at = usize::from(field.offset);
                let slot = &mut record[at..at + usize::from(field.size)];
                slot.fill(0);
                if !value.is_empty() {
                    adt::store_memo(field, Some(&value), slot, target)?;
                } else {
                    adt::store_memo(field, None, slot, target)?;
                }
            }
        }
        // A differential image's status byte only says deleted or not.
        record[0] = match (*deleted, encrypted) {
            (false, false) => adt::ACTIVE,
            (true, false) => adt::DELETED,
            (false, true) => adt::ENCRYPTED_ACTIVE,
            (true, true) => adt::ENCRYPTED_DELETED,
        };
        out.extend_from_slice(record);
    }
    let slots = kept.len() as u32;
    out[0x18..0x1C].copy_from_slice(&slots.to_le_bytes());
    out[0x28..0x2C].fill(0);
    out[0x30..0x34].fill(0);
    // The stamp is a differential backup's, which gives it after the copy.
    out[0x174..0x178].fill(0);
    out[0x58] |= adt::UNPREPARED as u8;
    fs::write(image, &out)?;
    crate::dd::create::free_table(image)?;
    Ok(())
}

/// Whether a field of one DBF flavour keeps its bytes in another: the types
/// the three flavours lay out alike, and the memo pointers, which are
/// written again.
fn converts(field: &Field) -> bool {
    use crate::field::FieldType::*;
    let plain = match &field.layout {
        crate::field::Layout::Dbf(layout) => layout.null_bit.is_none() && !layout.binary(),
        _ => false,
    };
    matches!(
        field.field_type,
        Character | Numeric | Date | Logical | Memo | Integer
    ) && (plain || matches!(field.field_type, Integer))
}

/// [`write_dbf`] for an image of another flavour than the source's: the
/// header and the records are laid out as the target flavour has them, the
/// memo values written again into a memo file of its kind.
fn write_dbf_converted(
    file: &mut TableFile,
    source: &Path,
    image: &Path,
    copy: &Copy,
    target: dbf::Flavour,
) -> Result<(), Error> {
    let header = file.structure.header.clone();
    if !file.structure.fields.iter().all(converts) {
        return Err(Error::Corrupt("field types the other flavour lacks"));
    }
    let specs: Vec<create::DbfFieldSpec> = file
        .structure
        .fields
        .iter()
        .map(create::DbfFieldSpec::from)
        .collect();
    let today = crate::value::Ymd {
        year: 2000,
        month: 1,
        day: 1,
    };
    let (mut bytes, _) = create::dbf_bytes(target, &specs, today);
    let (structure, _) = dbf::read_structure(&mut std::io::Cursor::new(&bytes[..]), target)?;
    let mut source_head = [0; 4];
    File::open(source)?.read_exact(&mut source_head)?;
    bytes[1..4].copy_from_slice(&source_head[1..4]);
    bytes.pop();
    let memo_kind = file.memo().map(|memo| memo.kind());
    let wanted = structure.fields.iter().any(Field::is_memo);
    let mut memo = None;
    if wanted {
        let kind = match target {
            dbf::Flavour::Clipper => MemoKind::Dbt,
            _ => MemoKind::Fpt,
        };
        let path = dbf::memo_path(image, kind);
        let block_size = match memo_kind {
            Some(MemoKind::Fpt) => file.memo_block_size().unwrap_or(64),
            _ => 64,
        };
        fs::write(
            &path,
            match kind {
                MemoKind::Dbt => create::dbt_bytes(),
                _ => create::fpt_bytes(block_size as u16),
            },
        )?;
        memo = Some(MemoFile::open_kind(&path, kind, true)?);
    }
    let record_length = usize::from(u16::from_le_bytes([bytes[10], bytes[11]]));
    let mut slots = 0u32;
    let mut out = bytes;
    for slot in 1..=header.record_slots {
        let record = match copy.overrides.get(&slot) {
            Some(None) => continue,
            Some(Some(before)) => before.clone(),
            None => file.record(slot)?,
        };
        slots += 1;
        let mut made = vec![0; record_length];
        made[0] = record[0];
        for (from, to) in file.structure.fields.clone().iter().zip(&structure.fields) {
            let (at, size) = (usize::from(from.offset), usize::from(from.size));
            if from.is_memo() {
                let source_memo = file.memo().ok_or(Error::CorruptMemo)?;
                let value = from
                    .memo_data(&record, Some(source_memo))?
                    .unwrap_or_default();
                to.encode_blank(&mut made)?;
                if let (false, Some(target)) = (value.is_empty(), memo.as_mut()) {
                    to.store_memo(Some(&value), &mut made, target)?;
                }
            } else {
                let to_at = usize::from(to.offset);
                made[to_at..to_at + size].copy_from_slice(&record[at..at + size]);
            }
        }
        out.extend_from_slice(&made);
    }
    out.push(dbf::END_OF_FILE);
    out[4..8].copy_from_slice(&slots.to_le_bytes());
    fs::write(image, &out)?;
    Ok(())
}

/// Copies the records of `slots` of the ADT table `source_file` into the
/// image at `image`, which a copy made with `keep_deleted` laid out slot for
/// slot, and gives the image `stamp`: what a `Diff` does for a table written
/// since the last backup. Memo values are written afresh into the image's memo
/// file.
pub fn write_changes(
    source_file: &mut TableFile,
    image: &Path,
    slots: &[u32],
    copy: &Copy,
    stamp: u32,
) -> Result<(), Error> {
    let header = source_file.structure.header.clone();
    let memo_fields = memo_fields(source_file);
    let key = copy
        .password
        .as_deref()
        .filter(|_| header.encrypted())
        .map(rc4_key);
    let record_length = header.record_length as usize;
    let data_offset = header.data_offset as usize;
    let mut bytes = fs::read(image)?;
    let mut count = u32::from_le_bytes(bytes[0x18..0x1C].try_into().unwrap());
    let mut memo = None;
    if !memo_fields.is_empty() && source_file.memo().is_some() {
        let mut target = MemoFile::open_adm(&MemoFile::path_for(image), true)?;
        target.set_counting(false);
        memo = Some(target);
    }
    let original_count = count;
    // The values the records had are given back when all are written, but
    // those of a record that is deleted now at once: a changed record's new
    // value takes a block of its own first (`fixtures/backup/s50_diff`).
    let mut later: Vec<(u32, Field, Vec<u8>)> = Vec::new();
    for &slot in slots {
        let mut record = vec![0; record_length];
        match copy.overrides.get(&slot) {
            Some(None) => continue,
            Some(Some(before)) => record.copy_from_slice(before),
            None => source_file.read_stored(slot, &mut record)?,
        }
        let deleted = matches!(source_file.status_of(&record), RecordStatus::Deleted);
        let at = data_offset + (slot as usize - 1) * record_length;
        // What the image has in the slot, if it is a record that has values.
        let mut old = vec![0; record_length];
        let had = slot <= original_count && at + record_length <= bytes.len();
        if had {
            old.copy_from_slice(&bytes[at..at + record_length]);
        }
        let had = had && old[0] & 1 == 0;
        if let Some(key) = key {
            crate::crypto::table_record_xor(key, slot, &mut record[adt::RECORD_PREFIX..]);
            crate::crypto::table_record_xor(key, slot, &mut old[adt::RECORD_PREFIX..]);
        }
        record[1..adt::RECORD_PREFIX].fill(0);
        if let Some(target) = memo.as_mut() {
            for field in &memo_fields {
                let place = usize::from(field.offset)..usize::from(field.offset + field.size);
                let old_ref = &old[place.clone()];
                if deleted {
                    if had {
                        if key.is_some() {
                            target.decipher_record(slot);
                        }
                        adt::free_memo(field, old_ref, target)?;
                    }
                    continue;
                }
                let source_memo = source_file.memo().ok_or(Error::CorruptMemo)?;
                if key.is_some() {
                    source_memo.decipher_record(slot);
                }
                let value = field
                    .memo_data(&record, Some(&mut *source_memo))?
                    .unwrap_or_default();
                // The value is written as the image's memo file would give a
                // changed record a place: the old one is freed, then the new
                // one takes the lowest free chunk of its size (its own again,
                // if no lower one is free: that is how a record whose value
                // is the same stays where it is). The first record is the
                // exception, whose old value goes back only after the pass.
                let old_block = u32::from_le_bytes(old_ref[0..4].try_into().unwrap());
                record[place.clone()].fill(0);
                if had && old_block != 0 {
                    match slot {
                        1 => later.push((slot, field.clone(), old_ref.to_vec())),
                        _ => {
                            if key.is_some() {
                                target.decipher_record(slot);
                            }
                            adt::free_memo(field, old_ref, target)?;
                        }
                    }
                }
                if key.is_some() {
                    target.decipher_record(slot);
                }
                let value = (!value.is_empty()).then_some(&value[..]);
                adt::store_memo(field, value, &mut record[place], target)?;
            }
        }
        record[0] = match (deleted, key.is_some()) {
            (false, false) => adt::ACTIVE,
            (true, false) => adt::DELETED,
            (false, true) => adt::ENCRYPTED_ACTIVE,
            (true, true) => adt::ENCRYPTED_DELETED,
        };
        if let Some(key) = key {
            crate::crypto::table_record_xor(key, slot, &mut record[adt::RECORD_PREFIX..]);
        }
        if bytes.len() < at + record_length {
            bytes.resize(at + record_length, 0);
        }
        bytes[at..at + record_length].copy_from_slice(&record);
        count = count.max(slot);
    }
    if let Some(target) = memo.as_mut() {
        for (slot, field, old) in later {
            if key.is_some() {
                target.decipher_record(slot);
            }
            adt::free_memo(&field, &old, target)?;
        }
    }
    bytes[0x18..0x1C].copy_from_slice(&count.to_le_bytes());
    bytes[0x174..0x178].copy_from_slice(&stamp.to_le_bytes());
    fs::write(image, &bytes)?;
    Ok(())
}

/// Copies the rows of `slots` of the dictionary file `source_file` into the
/// image of it at `image`, whole as they are (their memo is the `.am` the
/// caller copies), and gives the image `stamp`. The header is the source's
/// but for the free list, the flags and the stamp: the image keeps its own.
pub fn write_dictionary_changes(
    source_file: &mut TableFile,
    source: &Path,
    image: &Path,
    slots: &[u32],
    stamp: u32,
) -> Result<(), Error> {
    let header = source_file.structure.header.clone();
    let record_length = header.record_length as usize;
    let data_offset = header.data_offset as usize;
    let mut bytes = fs::read(image)?;
    let mut count = u32::from_le_bytes(bytes[0x18..0x1C].try_into().unwrap());
    for &slot in slots {
        let mut record = vec![0; record_length];
        source_file.read_stored(slot, &mut record)?;
        record[0] = adt::CHANGED | (record[0] & 1);
        let at = data_offset + (slot as usize - 1) * record_length;
        if bytes.len() < at + record_length {
            bytes.resize(at + record_length, 0);
        }
        bytes[at..at + record_length].copy_from_slice(&record);
        count = count.max(slot);
    }
    let mut head = vec![0; data_offset];
    File::open(source)?.read_exact(&mut head)?;
    for range in [
        0..0x28,
        0x2C..0x30,
        0x34..0x58,
        0x5C..0x174,
        0x178..data_offset,
    ] {
        bytes[range.clone()].copy_from_slice(&head[range]);
    }
    bytes[0x18..0x1C].copy_from_slice(&count.to_le_bytes());
    bytes[0x174..0x178].copy_from_slice(&stamp.to_le_bytes());
    fs::write(image, &bytes)?;
    Ok(())
}

/// Makes the copy of a dictionary file at `image` the base of differential
/// backups: every row marked changed, the flag [`adt::UNPREPARED`] set and
/// `stamp` stored.
pub fn stamp_dictionary(image: &Path, stamp: u32) -> Result<(), Error> {
    let mut bytes = fs::read(image)?;
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let slots = word(0x18);
    let record_length = usize::from(u16::from_le_bytes([bytes[0x24], bytes[0x25]]));
    let first = word(0x20);
    for slot in 0..slots {
        let at = first + slot * record_length;
        if bytes.get(at).is_some_and(|&byte| byte <= 0x07) {
            bytes[at] |= adt::CHANGED;
        }
    }
    bytes[0x58] |= adt::UNPREPARED as u8;
    bytes[0x174..0x178].copy_from_slice(&stamp.to_le_bytes());
    fs::write(image, &bytes)?;
    Ok(())
}

/// The backup stamp (header 0x174) of the ADT file at `path`.
pub fn read_stamp(path: &Path) -> Result<u32, Error> {
    let mut file = File::open(path)?;
    let mut bytes = [0; 4];
    file.seek(SeekFrom::Start(adt::offsets::BACKUP_STAMP))?;
    file.read_exact(&mut bytes)
        .map_err(|_| Error::Corrupt("file ends inside the header"))?;
    Ok(u32::from_le_bytes(bytes))
}

/// Gives the ADT image at `path` the backup stamp `stamp`.
pub fn write_stamp(path: &Path, stamp: u32) -> Result<(), Error> {
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.seek(SeekFrom::Start(adt::offsets::BACKUP_STAMP))?;
    file.write_all(&stamp.to_le_bytes())?;
    Ok(())
}

/// Whether the image at `image` has the fields and record length of the
/// table `source_file`: a differential backup copies records into one that
/// does, and writes the table afresh otherwise.
pub fn same_structure(source_file: &TableFile, image: &Path) -> bool {
    TableFile::open(image, Format::Adt).is_ok_and(|image| {
        image.structure.header.record_length == source_file.structure.header.record_length
            && image.structure.fields == source_file.structure.fields
    })
}

fn write_dbf(file: &mut TableFile, source: &Path, image: &Path, copy: &Copy) -> Result<(), Error> {
    if let (Format::Dbf(own), Some(target)) = (file.structure.header.format, copy.flavour)
        && own != target
    {
        return write_dbf_converted(file, source, image, copy, target);
    }
    let header = file.structure.header.clone();
    let memo_fields = memo_fields(file);
    let kind = file.memo().map(|memo| memo.kind());
    fs::copy(source, image)?;
    let mut out = OpenOptions::new().read(true).write(true).open(image)?;
    // The structural index is not copied.
    let mut flags = [0];
    out.seek(SeekFrom::Start(0x1C))?;
    out.read_exact(&mut flags)?;
    out.seek(SeekFrom::Start(0x1C))?;
    out.write_all(&[flags[0] & !0x01])?;
    let mut target = None;
    if let Some(kind) = kind.filter(|_| !memo_fields.is_empty()) {
        let block_size = file.memo_block_size().unwrap_or(512);
        let memo_path = dbf::memo_path(image, kind);
        let bytes = match kind {
            MemoKind::Dbt => create::dbt_bytes(),
            _ => create::fpt_bytes(block_size as u16),
        };
        fs::write(&memo_path, bytes)?;
        target = Some(MemoFile::open_kind(&memo_path, kind, true)?);
    }
    let mut kept = 0;
    for slot in 1..=header.record_slots {
        let mut record = match copy.overrides.get(&slot) {
            // A record a transaction not ended yet appended: it is at the end
            // of the file, and not in the image.
            Some(None) => continue,
            Some(Some(before)) => before.clone(),
            None => file.record(slot)?,
        };
        kept = slot;
        if let Some(target) = target.as_mut() {
            let mut values = Vec::with_capacity(memo_fields.len());
            for field in &memo_fields {
                let memo = file.memo().ok_or(Error::CorruptMemo)?;
                values.push(field.memo_data(&record, Some(memo))?.unwrap_or_default());
            }
            for (field, value) in memo_fields.iter().zip(values) {
                field.encode_blank(&mut record)?;
                if !value.is_empty() {
                    field.store_memo(Some(&value), &mut record, target)?;
                }
            }
        }
        let at =
            u64::from(header.data_offset) + u64::from(slot - 1) * u64::from(header.record_length);
        out.seek(SeekFrom::Start(at))?;
        out.write_all(&record)?;
    }
    if kept < header.record_slots {
        out.seek(SeekFrom::Start(4))?;
        out.write_all(&kept.to_le_bytes())?;
        let end = u64::from(header.data_offset) + u64::from(kept) * u64::from(header.record_length);
        out.set_len(end + 1)?;
        out.seek(SeekFrom::Start(end))?;
        out.write_all(&[0x1A])?;
    }
    Ok(())
}

/// Turns the image of an ADT table at `table` into a table of the dictionary
/// file `dictionary_file` (its name) in which the table has the object `id`.
pub fn bind_image(table: &Path, id: u32, dictionary_file: &str) -> Result<(), Error> {
    crate::dd::create::bind_table(table, id, dictionary_file)?;
    Ok(())
}

/// Gives a DBF image its structural index bit back, which a restore does
/// once it has rebuilt the table's `.cdx`.
pub fn mark_structural(table: &Path) -> Result<(), Error> {
    let mut file = OpenOptions::new().read(true).write(true).open(table)?;
    let mut flags = [0];
    file.seek(SeekFrom::Start(0x1C))?;
    file.read_exact(&mut flags)?;
    file.seek(SeekFrom::Start(0x1C))?;
    file.write_all(&[flags[0] | 0x01])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str) -> std::path::PathBuf {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        fixtures.join("fixtures/backup/s01_full/tree").join(name)
    }

    #[test]
    fn images_match_the_originals() {
        let out = crate::testing::temp_root().join("image-s01");
        let _ = fs::remove_dir_all(&out);
        fs::create_dir_all(&out).unwrap();
        for (name, format) in [
            ("t_adt.adt", Format::Adt),
            ("t_cdx.dbf", Format::Dbf(dbf::Flavour::FoxPro)),
            ("t_vfp.dbf", Format::Dbf(dbf::Flavour::Visual)),
            ("t_ntx.dbf", Format::Dbf(dbf::Flavour::Clipper)),
        ] {
            let source = tree("src").join(name);
            let mut file = TableFile::open(&source, format).unwrap();
            write_image(&mut file, &source, &out.join(name), &Copy::default()).unwrap();
        }
        if let Ok(keep) = std::env::var("KEEP_IMAGES") {
            let _ = fs::remove_dir_all(&keep);
            fs::create_dir_all(&keep).unwrap();
            for entry in fs::read_dir(&out).unwrap() {
                let entry = entry.unwrap();
                fs::copy(entry.path(), Path::new(&keep).join(entry.file_name())).unwrap();
            }
        }
        for entry in fs::read_dir(tree("img")).unwrap() {
            let name = entry.unwrap().file_name();
            if name.to_string_lossy().starts_with("d.") {
                continue;
            }
            let want = fs::read(tree("img").join(&name)).unwrap();
            let got = fs::read(out.join(&name)).unwrap();
            let first = want.iter().zip(&got).position(|(a, b)| a != b);
            assert!(
                want == got,
                "{name:?}: sizes {} {}, first difference {first:?}",
                want.len(),
                got.len()
            );
        }
    }
}
