//! `PrepareDiff` and `Diff` of `sp_BackupDatabase` (`fixtures/backup/README.md`,
//! "Differential"): the first writes a full image that keeps deleted records
//! and clears the source's changed marks; a `Diff` copies the records marked
//! since into the image it finds, or writes the table afresh where that image
//! is missing or no base for it.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use tflip_storage::image;
use tflip_storage::table::{Format, TableFile};

use super::database::{copy_table, copy_table_with, open_code};
use super::rows::{Notice, Rows};
use crate::table::TableType;
use crate::{Error, shared};

/// Which of the two a call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Prepare,
    Diff,
}

/// The mode the options ask for; `Diff` wins where both are given.
pub(crate) fn mode_of(options: &super::options::Options) -> Option<Mode> {
    match (options.prepare_diff, options.diff) {
        (_, true) => Some(Mode::Diff),
        (true, _) => Some(Mode::Prepare),
        _ => None,
    }
}

/// The stamp a call gives its source and image: the time in seconds, as the
/// original's is (two calls in one second give the same).
pub(crate) fn stamp() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |time| time.as_secs() as u32)
}

/// What a call of the procedure does to every table.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Call {
    pub mode: Mode,
    pub stamp: u32,
}

/// Why a `Diff` writes an image afresh.
enum Reason {
    Missing,
    Structure,
    Stamp,
}

impl Reason {
    fn notice(&self, table: String) -> Notice {
        match self {
            Reason::Missing => Notice::NewImage { table },
            Reason::Structure => Notice::Reinitialised {
                table,
                structure: true,
            },
            Reason::Stamp => Notice::Reinitialised {
                table,
                structure: false,
            },
        }
    }
}

/// Whether the stamps of source and image make the image a base for a copy of
/// changed records: a full image has none, and neither has a table that was
/// rewritten since.
fn matches(source: u32, image: u32) -> bool {
    source != 0 && source == image
}

fn storage_code(error: tflip_storage::table::Error) -> u32 {
    open_code(&error)
}

/// The dictionary file's part of the call, after the dictionary's own files
/// were copied to the image by a `PrepareDiff` (not by a `Diff`, which this
/// does the copying of).
pub(crate) fn dictionary(
    add: &Path,
    image_add: &Path,
    row_name: String,
    mode: Mode,
    stamp: u32,
    rows: &mut Rows,
) -> Result<(), Error> {
    let corrupt = |_| Error::CorruptTable;
    let mut source = TableFile::open_dictionary_for_writing(add).map_err(corrupt)?;
    let memo = tflip_storage::dd::memo_path(add);
    let image_memo = tflip_storage::dd::memo_path(image_add);
    let mut copied = true;
    let copy_whole = |rows: &mut Rows, reason: Option<Reason>| -> Result<(), Error> {
        if let Some(reason) = reason {
            rows.push(reason.notice(row_name.clone()));
        }
        fs::copy(add, image_add).map_err(|_| Error::FileCreate)?;
        if memo.exists() {
            fs::copy(&memo, &image_memo).map_err(|_| Error::FileCreate)?;
        }
        image::stamp_dictionary(image_add, stamp).map_err(corrupt)
    };
    match mode {
        Mode::Prepare => copy_whole(rows, None)?,
        Mode::Diff => {
            let image_stamp = image_add
                .is_file()
                .then(|| image::read_stamp(image_add).unwrap_or(0));
            let source_stamp = source.backup_stamp().map_err(corrupt)?;
            match image_stamp {
                None => copy_whole(rows, Some(Reason::Missing))?,
                Some(image_stamp) if !matches(source_stamp, image_stamp) => {
                    copy_whole(rows, Some(Reason::Stamp))?
                }
                Some(_) if source.written_since_backup() => {
                    let slots = source.changed_slots().map_err(corrupt)?;
                    image::write_dictionary_changes(&mut source, add, image_add, &slots, stamp)
                        .map_err(corrupt)?;
                    if memo.exists() {
                        fs::copy(&memo, &image_memo).map_err(|_| Error::FileCreate)?;
                    }
                }
                Some(_) => {
                    copied = false;
                    image::write_stamp(image_add, stamp).map_err(corrupt)?
                }
            }
        }
    }
    leave(&mut source, copied, stamp).map_err(corrupt)
}

/// What the call leaves in the source once its image has the changes: the
/// marks cleared if it copied any (or the table in full), the stamp either way.
fn leave(
    source: &mut TableFile,
    copied: bool,
    stamp: u32,
) -> Result<(), tflip_storage::table::Error> {
    match copied {
        true => source.clear_changed(stamp),
        false => source.set_backup_stamp(stamp),
    }
}

/// The part of the call for the dictionary's table `table`, whose image is
/// `target` (`row_name` names it in rows). An ADT table is copied as the mode
/// says; the others, which keep no marks, are copied in full each time.
/// Whether the image was written whole, not only given the changes.
pub(crate) fn table(
    table: &Path,
    target: &Path,
    table_type: TableType,
    password: Option<&[u8]>,
    call: Call,
    row_name: String,
    rows: &mut Rows,
) -> Result<bool, u32> {
    let Call { mode, stamp } = call;
    if !matches!(table_type, TableType::Adt | TableType::Database) {
        return copy_table(table, target, table_type, password).map(|()| true);
    }
    let shared = shared::open(table, Format::Adt, password).map_err(storage_code)?;
    let mut source = shared.file();
    let copy = image::Copy {
        keep_deleted: true,
        password: password.map(<[u8]>::to_vec),
        overrides: shared::committed_view(table),
        flavour: None,
    };
    let reason = match mode {
        Mode::Prepare => Some(None),
        Mode::Diff if !target.is_file() => Some(Some(Reason::Missing)),
        Mode::Diff if !image::same_structure(&source, target) => Some(Some(Reason::Structure)),
        Mode::Diff => {
            let source_stamp = source.backup_stamp().map_err(storage_code)?;
            let image_stamp = image::read_stamp(target).map_err(storage_code)?;
            (!matches(source_stamp, image_stamp)).then_some(Some(Reason::Stamp))
        }
    };
    let mut copied = true;
    let mut whole = false;
    match reason {
        Some(reason) => {
            whole = true;
            if let Some(reason) = reason {
                rows.push(reason.notice(row_name));
            }
            copy_table_with(table, target, table_type, &copy)?;
            image::write_stamp(target, stamp).map_err(storage_code)?;
        }
        None if source.written_since_backup() => {
            let slots = source.changed_slots().map_err(storage_code)?;
            let mut reader = TableFile::open_deciphering(table, Format::Adt, false, password)
                .map_err(storage_code)?;
            image::write_changes(&mut reader, target, &slots, &copy, stamp)
                .map_err(storage_code)?;
        }
        None => {
            copied = false;
            image::write_stamp(target, stamp).map_err(storage_code)?
        }
    }
    leave(&mut source, copied, stamp).map_err(storage_code)?;
    Ok(whole)
}

/// Turns the copy of a differential image's table at `table` into a table:
/// the image keeps the records that were deleted, a restore does not
/// (`fixtures/backup/s50_diff`, `dst`). A full image has no stamp and is left
/// as it is.
pub(crate) fn unpack(table: &Path, password: Option<&[u8]>) -> Result<(), u32> {
    if image::read_stamp(table).map_err(storage_code)? == 0 {
        return Ok(());
    }
    let packed = table.with_extension("tflip.adt");
    let copy = image::Copy {
        password: password.map(<[u8]>::to_vec),
        ..image::Copy::default()
    };
    copy_table_with(table, &packed, TableType::Adt, &copy)?;
    let memo = tflip_storage::memo::MemoFile::path_for(&packed);
    let swapped = fs::rename(&packed, table).and_then(|()| match memo.exists() {
        true => fs::rename(&memo, tflip_storage::memo::MemoFile::path_for(table)),
        false => Ok(()),
    });
    swapped.map_err(|_| 7008)
}
