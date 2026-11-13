//! Renaming objects and moving their files.

use super::*;

/// Renames a dictionary object (`AdsDDRenameObject`), as `sp_RenameDDObject`
/// does: the two share their checks and what they write, a table's files
/// included unless `options` keeps them.
pub fn rename_object_api(
    session: &Session,
    connection: Handle,
    name: &str,
    new_name: &str,
    object_type: u16,
    options: u32,
) -> Result<(), Error> {
    crate::procedure::rename_dd_object(session, connection, name, new_name, object_type, options)
}

/// A dictionary path (`.\\name`) without its `.\\` prefix.
pub(crate) fn strip_relative(path: &str) -> &str {
    path.strip_prefix(".\\")
        .or_else(|| path.strip_prefix("./"))
        .unwrap_or(path)
}

/// Resolves a dictionary-relative path (`\\`- or `/`-separated, a leading `.`
/// dropped) to a file under `folder`, as [`crate::dictionary::Dictionary::path`]
/// does when the dictionary reads a table's or index file's location.
pub(crate) fn under_folder(folder: &Path, relative: &str) -> PathBuf {
    let mut path = folder.to_path_buf();
    for part in relative
        .split(['\\', '/'])
        .filter(|p| !p.is_empty() && *p != ".")
    {
        path.push(part);
    }
    path
}

/// A 5020 (`AE_INVALID_PATH`) error worded as the original engine words it for a
/// move: `detail` is the specific cause.
fn move_path_error(detail: String) -> Error {
    ace_error::sql(5020, &detail)
}

/// The id-5132 "no such named `what`" error the engine answers `AdsDDMoveObjectFile`.
fn move_unknown(what: &str) -> Error {
    ace_error::sql(
        5132,
        &format!("There is no such named {what} in the data dictionary."),
    )
}

/// One file to relocate: its source and destination on disk, and, when it is a
/// dictionary object (the table or an index file), the object's record and the
/// new `.\\...` relative path to store in its `RELATIVE_PATH` slot.
struct MoveFile {
    src: PathBuf,
    dst: PathBuf,
    slot: Option<(u32, Vec<u8>)>,
}

/// The relative path the engine stores for a file moved into `dest_dir`: `.\\`
/// and the directory relative to the dictionary's `folder` when the destination
/// is under it (as it usually is), otherwise the destination kept whole, with
/// the file name appended and a trailing NUL — as `AdsDDMoveObjectFile` writes it.
fn stored_relative(folder: &Path, dest_dir: &Path, file_name: &str, typed: &str) -> String {
    // A destination given as a relative path is kept as it was given.
    let typed = typed.replace('/', "\\");
    if !typed.is_empty() && !Path::new(&typed).is_absolute() && !typed.starts_with('\\') {
        return format!("{}\\{file_name}\0", typed.trim_end_matches('\\'));
    }
    match dest_dir.strip_prefix(folder) {
        Ok(rel) if rel.as_os_str().is_empty() => format!(".\\{file_name}\0"),
        Ok(rel) => format!(
            ".\\{}\\{}\0",
            rel.to_string_lossy().replace('/', "\\"),
            file_name
        ),
        Err(_) => format!(
            "{}\\{}\0",
            dest_dir.to_string_lossy().replace('/', "\\"),
            file_name
        ),
    }
}

/// Plans the move of one index file (by its object name) into `dest_dir`: finds
/// the index-file object, resolves its current file and the new relative path.
fn plan_index_move(
    dict: &tflip_storage::dd::Dictionary,
    folder: &Path,
    dest_dir: &Path,
    name: &str,
    typed: &str,
) -> Result<MoveFile, Error> {
    use tflip_storage::dd::{self, Kind};
    let object = dict
        .named(Kind::IndexFile, name)
        .ok_or_else(|| move_unknown("index file"))?;
    let rel = object
        .text(dd::index_file::RELATIVE_PATH)
        .unwrap_or_default();
    let src = under_folder(folder, &rel);
    let file_name = src
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or(Error::CorruptTable)?
        .to_owned();
    let new_rel = stored_relative(folder, dest_dir, &file_name, typed);
    Ok(MoveFile {
        dst: dest_dir.join(&file_name),
        src,
        slot: Some((object.record(), new_rel.into_bytes())),
    })
}

/// Moves a table's files — its `.adt`, its `.adm` memo if it has one, and the
/// index files named (`;`-separated) in `index_files` — or a single index file
/// (`object_type` [`ADS_DD_INDEX_FILE_OBJECT`], 3) to `new_path`, updating each
/// moved object's stored relative path. This is `AdsDDMoveObjectFile`.
///
/// `new_path` is the destination directory (relative to the dictionary's folder,
/// or absolute). As the original engine does, an unknown table or index file
/// fails with `AE_INVALID_OBJECT_NAME` (5132); a destination that does not exist,
/// or a destination file that already exists, fails with `AE_INVALID_PATH`
/// (5020) and the engine's own wording (probed black-box; see `docs/ADS_BUGS.md`).
/// Only ADT tables are handled; another table type is [`Error::NotImplemented`].
#[allow(clippy::too_many_arguments)]
pub fn move_object_file_api(
    session: &Session,
    connection: Handle,
    object_type: u16,
    object_name: &str,
    new_path: &str,
    index_files: &str,
    _parent: &str,
    _options: u32,
) -> Result<(), Error> {
    use tflip_storage::dd::{self, Kind};
    let (conn, add_path) = dictionary_connection(session, connection)?;
    let folder = dictionary_folder(&add_path).to_path_buf();
    let dict = open_dict(&add_path)?;

    // The destination directory: relative to the dictionary's folder, or absolute.
    let normalized = new_path.replace('\\', "/");
    let as_path = Path::new(&normalized);
    let dest_dir = if as_path.is_absolute() {
        as_path.to_path_buf()
    } else {
        under_folder(&folder, &normalized)
    };

    // Collect the files to move and the slots to rewrite, then drop the read of
    // the dictionary before touching disk.
    let mut files: Vec<MoveFile> = Vec::new();
    match object_type {
        1 /* ADS_DD_TABLE_OBJECT */ => {
            let table = dict.named(Kind::Table, object_name).ok_or_else(|| move_unknown("table"))?;
            let rel = table.text(dd::table::RELATIVE_PATH).unwrap_or_default();
            let src = under_folder(&folder, &rel);
            let file_name =
                src.file_name().and_then(|f| f.to_str()).ok_or(Error::CorruptTable)?.to_owned();
            if !file_name.to_ascii_lowercase().ends_with(".adt") {
                return Err(Error::NotImplemented);
            }
            let new_rel = stored_relative(&folder, &dest_dir, &file_name, new_path);
            // The memo file (<base>.adm), moved with the data but not a dictionary
            // object, so it carries no slot.
            let memo = src.with_extension("adm");
            if memo.exists() {
                let memo_name = memo.file_name().and_then(|f| f.to_str()).unwrap_or_default();
                files.push(MoveFile { dst: dest_dir.join(memo_name), src: memo, slot: None });
            }
            files.push(MoveFile {
                dst: dest_dir.join(&file_name),
                src,
                slot: Some((table.record(), new_rel.into_bytes())),
            });
            for name in index_files.split(';').map(str::trim).filter(|n| !n.is_empty()) {
                files.push(plan_index_move(&dict, &folder, &dest_dir, name, new_path)?);
            }
        }
        3 /* ADS_DD_INDEX_FILE_OBJECT */ => {
            files.push(plan_index_move(&dict, &folder, &dest_dir, object_name, new_path)?);
        }
        _ => return Err(Error::NotImplemented),
    }
    drop(dict);

    // Validate before moving anything: the destination exists and no destination
    // file is already there — the engine moves nothing on either failure.
    if !dest_dir.is_dir() {
        return Err(move_path_error(
            "Error moving table or index, error code: 3.".to_owned(),
        ));
    }
    for file in &files {
        if file.dst.exists() {
            return Err(move_path_error(format!(
                "Error moving table or index, destination file already exists: {}",
                file.dst.display()
            )));
        }
    }
    for file in &files {
        crate::cache::evict_path(&file.src);
        std::fs::rename(&file.src, &file.dst).map_err(|_| {
            move_path_error("Error moving table or index, error code: 3.".to_owned())
        })?;
    }
    // A table's file names its dictionary from where it is.
    let dictionary_file = add_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for file in &files {
        if file
            .dst
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("adt"))
        {
            let reference = match dest_dir.strip_prefix(&folder) {
                Ok(rel) if rel.as_os_str().is_empty() => format!(".\\{dictionary_file}"),
                Ok(rel) => format!(
                    "{}{dictionary_file}",
                    "..\\".repeat(rel.components().count())
                ),
                Err(_) => add_path.display().to_string(),
            };
            dd::create::set_back_reference(&file.dst, &reference)
                .map_err(|_| Error::CorruptTable)?;
        }
    }
    for file in &files {
        if let Some((record, new_rel)) = &file.slot {
            set_object_slot(
                session,
                &conn,
                &add_path,
                *record,
                dd::table::RELATIVE_PATH,
                new_rel,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn moves_a_table_and_its_index_file_like_the_engine() {
        use tflip_storage::dd::{self, Dictionary, Kind};
        let session = Session::new();
        let fx = ops_fixtures();
        let dir = tflip_storage::testing::temp_root()
            .join(format!("tflip-ddmove-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let add = dir.join("db.add");

        let connect = create_dictionary_api(&session, &add.to_string_lossy(), 0).unwrap();
        copy_free(&fx.join("t.adt"), &dir.join("t.adt"));
        std::fs::copy(fx.join("ix.adi"), dir.join("ix.adi")).unwrap();
        add_table_api(&session, connect, "t", "t.adt", TableType::Adt, 1, "", "").unwrap();
        add_index_file_api(&session, connect, "t", "ix.adi", "").unwrap();
        let index_file = {
            let dict = Dictionary::open(&add).unwrap();
            dict.all(Kind::IndexFile)[0].name.clone()
        };

        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        move_object_file_api(
            &session,
            connect,
            1, // ADS_DD_TABLE_OBJECT
            "t",
            &sub.to_string_lossy(),
            &index_file,
            "",
            0,
        )
        .unwrap();

        // The data and index files moved; the originals are gone.
        assert!(sub.join("t.adt").exists(), "t.adt moved");
        assert!(sub.join(&index_file).exists(), "index file moved");
        assert!(
            !dir.join("t.adt").exists(),
            "t.adt no longer in the old place"
        );
        assert!(
            !dir.join(&index_file).exists(),
            "index file gone from the old place"
        );

        // The dictionary's stored relative paths point at the new location,
        // `.\sub\...`, as the engine writes them (probed black-box).
        {
            let dict = Dictionary::open(&add).unwrap();
            assert_eq!(
                dict.named(Kind::Table, "t")
                    .unwrap()
                    .text(dd::table::RELATIVE_PATH)
                    .as_deref(),
                Some(".\\sub\\t.adt"),
            );
            let moved = dict.named(Kind::IndexFile, &index_file).unwrap();
            assert_eq!(
                moved.text(dd::index_file::RELATIVE_PATH),
                Some(format!(".\\sub\\{index_file}")),
            );
        }

        // An unknown table is refused with 5132, a destination that does not
        // exist with 5020 — the engine's codes.
        match move_object_file_api(
            &session,
            connect,
            1,
            "nope",
            &sub.to_string_lossy(),
            "",
            "",
            0,
        ) {
            Err(Error::Sql(e)) => assert_eq!(e.native, 5132),
            other => panic!("expected 5132, got {other:?}"),
        }
        let missing = dir.join("nope");
        match move_object_file_api(
            &session,
            connect,
            1,
            "t",
            &missing.to_string_lossy(),
            "",
            "",
            0,
        ) {
            Err(Error::Sql(e)) => assert_eq!(e.native, 5020),
            other => panic!("expected 5020, got {other:?}"),
        }

        connection::disconnect(&session, connect).ok();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
