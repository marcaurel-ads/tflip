//! `sp_BackupFreeTables` and `sp_RestoreFreeTables`: the images of the free
//! tables of a folder, and the tables made from one (`fixtures/backup/s40`,
//! `s41`, `s61`, `s83`).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use tflip_storage::dbf::Flavour;
use tflip_storage::image;
use tflip_storage::memo::MemoFile;
use tflip_storage::table::{Format, TableFile};

use super::database;
use super::differential;
use super::options::Options;
use super::paths;
use super::rows::{Notice, Rows};
use crate::table::{TableSettings, TableType};
use crate::{Error, Session, connection, shared};

/// The passwords a call gives for its encrypted tables.
enum Passwords {
    None,
    /// One password for every table.
    One(String),
    /// `name.ext=password` pairs, the names compared whatever their case.
    Listed(Vec<(String, String)>),
}

/// What the passwords say about one table.
enum Lookup {
    /// The call gave no password at all.
    Missing,
    /// It gave passwords, and none for this table.
    NotListed,
    Given(String),
}

impl Passwords {
    fn parse(text: Option<&str>) -> Passwords {
        match text.filter(|text| !text.is_empty()) {
            None => Passwords::None,
            Some(text) if text.contains('=') => Passwords::Listed(
                text.split(';')
                    .filter_map(|pair| pair.split_once('='))
                    .map(|(name, password)| (name.trim().to_ascii_lowercase(), password.to_owned()))
                    .collect(),
            ),
            Some(text) => Passwords::One(text.to_owned()),
        }
    }

    fn lookup(&self, file_name: &str) -> Lookup {
        match self {
            Passwords::None => Lookup::Missing,
            Passwords::One(password) => Lookup::Given(password.clone()),
            Passwords::Listed(pairs) => pairs
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(file_name))
                .map_or(Lookup::NotListed, |(_, password)| {
                    Lookup::Given(password.clone())
                }),
        }
    }
}

/// The type `TableTypeMap` gives the extension `extension`, if it names one.
fn mapped_type(map: Option<&str>, extension: &str) -> Option<TableType> {
    map?.split(',').find_map(|pair| {
        let (name, kind) = pair.split_once('=')?;
        if !name
            .trim()
            .trim_start_matches('.')
            .eq_ignore_ascii_case(extension)
        {
            return None;
        }
        match kind.trim().to_ascii_uppercase().as_str() {
            "ADS_ADT" => Some(TableType::Adt),
            "ADS_CDX" => Some(TableType::Cdx),
            "ADS_VFP" => Some(TableType::Vfp),
            "ADS_NTX" => Some(TableType::Ntx),
            _ => None,
        }
    })
}

/// The extension of the file name `name`, empty if it has none.
fn extension_of(name: &str) -> &str {
    name.rsplit_once('.').map_or("", |(_, extension)| extension)
}

/// Whether `name` matches `mask`, which has `*` and `?`; whatever the case.
fn matches(mask: &str, name: &str) -> bool {
    fn go(mask: &[char], name: &[char]) -> bool {
        match mask.split_first() {
            None => name.is_empty(),
            Some(('*', rest)) => (0..=name.len()).any(|skip| go(rest, &name[skip..])),
            Some(('?', rest)) => !name.is_empty() && go(rest, &name[1..]),
            Some((letter, rest)) => name
                .split_first()
                .is_some_and(|(first, tail)| first.eq_ignore_ascii_case(letter) && go(rest, tail)),
        }
    }
    let mask: Vec<char> = mask.chars().collect();
    let name: Vec<char> = name.chars().collect();
    go(&mask, &name)
}

/// The files of `folder`, by name.
fn files_of(folder: &Path) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            )
        })
        .collect();
    found.sort_by_key(|(name, _)| name.to_ascii_lowercase());
    found
}

/// Why a file is not backed up: the code and what the row adds to its text.
struct Refusal {
    code: u32,
    detail: String,
}

impl Refusal {
    fn at(code: u32, path: &str) -> Refusal {
        Refusal {
            code,
            detail: path.to_owned(),
        }
    }
}

fn open_code(error: &tflip_storage::table::Error) -> u32 {
    use tflip_storage::table::Error as E;
    match error {
        E::Io(error) if error.kind() == std::io::ErrorKind::NotFound => 7041,
        E::Io(_) => 7008,
        E::Encrypted | E::EncryptedDescriptors => 7060,
        _ => 7016,
    }
}

/// The flavour a DBF table's version byte says it is: `None` for a version
/// that fits more than one, an error for a file that is no DBF table.
fn flavour_of(path: &Path) -> Result<Option<Flavour>, ()> {
    let mut version = [0];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut version))
        .map_err(|_| ())?;
    match version[0] {
        0x83 | 0x87 => Ok(Some(Flavour::Clipper)),
        0xF5 | 0xF7 => Ok(Some(Flavour::FoxPro)),
        0x30..=0x32 | 0xF8 | 0xF9 => Ok(Some(Flavour::Visual)),
        0x03 | 0x07 => Ok(None),
        _ => Err(()),
    }
}

/// What a free-table backup is asked for.
pub(crate) struct Backup<'a> {
    pub source: &'a str,
    pub mask: &'a str,
    pub dest: &'a str,
    pub options: &'a Options,
    pub passwords: Option<&'a str>,
    /// The type of a table the options map no type for: the connection's.
    pub default_type: TableType,
}

/// One file to back up.
struct Entry {
    /// What the rows call it: the path as the call gave it.
    given: String,
    name: String,
    path: PathBuf,
    table_type: TableType,
}

/// Backs up the free tables `backup.mask` names in the folder
/// `backup.source` into the folder `backup.dest`.
pub(crate) fn backup(backup: &Backup) -> Result<Rows, Error> {
    let Backup {
        source,
        mask,
        dest,
        options,
        ..
    } = *backup;
    let mut rows = Rows::default();
    let folder = paths::local(source).filter(|folder| folder.is_dir());
    let listed = folder.as_deref().map(files_of).unwrap_or_default();
    let type_of = |name: &str| {
        mapped_type(options.table_type_map.as_deref(), extension_of(name))
            .unwrap_or(backup.default_type)
    };
    let excluded = |name: &str| {
        options
            .exclude
            .iter()
            .any(|item| item.eq_ignore_ascii_case(name))
    };
    let mut entries: Vec<Entry> = Vec::new();
    for mask in mask
        .split(';')
        .map(str::trim)
        .filter(|mask| !mask.is_empty())
    {
        for (name, path) in &listed {
            if matches(mask, name)
                && !entries
                    .iter()
                    .any(|entry| entry.name.eq_ignore_ascii_case(name))
            {
                entries.push(Entry {
                    given: paths::join(source, name),
                    name: name.clone(),
                    path: path.clone(),
                    table_type: type_of(name),
                });
            }
        }
    }
    if entries.is_empty() || options.meta_only {
        rows.push(Notice::NoSourceTables {
            path: format!("{}\\", source.trim_end_matches(['\\', '/'])),
        });
        return Ok(rows);
    }
    let dest_folder = paths::local(dest);
    let passwords = Passwords::parse(backup.passwords);
    let mut done: Vec<String> = Vec::new();
    // The tables the include list names are backed up after those of the mask,
    // as the mask's own, with the type their extension maps to.
    let mut work: Vec<(Entry, bool)> = entries
        .into_iter()
        .filter(|entry| !excluded(&entry.name))
        .map(|entry| (entry, false))
        .collect();
    for item in options.include.iter().flatten() {
        let table_type = type_of(item);
        let name = match item.contains('.') {
            true => item.clone(),
            false => format!("{item}.{}", extension_for(table_type)),
        };
        if excluded(&name) {
            continue;
        }
        let path = folder
            .as_deref()
            .and_then(|folder| paths::find(folder, &name))
            .or_else(|| folder.as_deref().map(|folder| folder.join(&name)))
            .unwrap_or_else(|| PathBuf::from(&name));
        let name = path
            .file_name()
            .map_or(name.clone(), |name| name.to_string_lossy().into_owned());
        work.push((
            Entry {
                given: paths::join(source, item),
                name,
                path,
                table_type,
            },
            true,
        ));
    }
    // What the backup wrote, in image order, for an archive.
    let mut written: Vec<PathBuf> = Vec::new();
    let call = differential::mode_of(options).map(|mode| differential::Call {
        mode,
        stamp: differential::stamp(),
    });
    for (entry, included) in work {
        let lower = entry.name.to_ascii_lowercase();
        // Only the second attempt on a table that is not an ADT one is told
        // that the table is being backed up.
        if included && done.contains(&lower) && entry.table_type != TableType::Adt {
            rows.push(Notice::ListedTwice { table: entry.name });
            continue;
        }
        done.push(lower);
        let Some(dest_folder) = dest_folder.as_deref() else {
            rows.push(Notice::OpenForBackup {
                table: entry.given.clone(),
                code: 7008,
                detail: Some(entry.given),
                free: true,
            });
            continue;
        };
        if one(&entry, dest, dest_folder, &passwords, call, &mut rows) {
            written.extend(image_files(dest_folder, &entry.name));
        }
    }
    if let (Some(archive), Some(folder)) = (&options.archive, dest_folder.as_deref()) {
        database::archive_image(folder, dest, archive, &written, &mut rows);
    }
    Ok(rows)
}

fn extension_for(table_type: TableType) -> &'static str {
    match table_type {
        TableType::Adt | TableType::Database => "adt",
        _ => "dbf",
    }
}

/// The files an image of the table `name` made in `folder`: the table, its
/// memo file and its index file, in the order an archive holds them.
fn image_files(folder: &Path, name: &str) -> Vec<PathBuf> {
    let stem = stem_of(name);
    let own = [extension_of(name), "adm", "fpt", "dbt", "adi", "cdx"];
    own.iter()
        .filter_map(|extension| paths::find(folder, &format!("{stem}.{extension}")))
        .collect()
}

/// Backs up the one file `entry`, with the rows it earns; whether it wrote an
/// image.
fn one(
    entry: &Entry,
    dest: &str,
    dest_folder: &Path,
    passwords: &Passwords,
    call: Option<differential::Call>,
    rows: &mut Rows,
) -> bool {
    let refuse = |rows: &mut Rows, refusal: Refusal| {
        rows.push(Notice::OpenForBackup {
            table: entry.given.clone(),
            code: refusal.code,
            detail: Some(refusal.detail),
            free: true,
        });
    };
    let path_text = paths::join(paths::folder_of(&entry.given), &entry.name);
    if !entry.path.exists() {
        refuse(rows, Refusal::at(7041, &path_text));
        return false;
    }
    if shared::held_exclusively(&entry.path)
        || matches!(
            extension_of(&entry.name).to_ascii_lowercase().as_str(),
            "fpt" | "dbt"
        )
    {
        refuse(rows, Refusal::at(7008, &path_text));
        return false;
    }
    let target = dest_folder.join(&entry.name);
    let copy = image::Copy {
        overrides: shared::committed_view(&entry.path),
        ..image::Copy::default()
    };
    let written = match entry.table_type {
        TableType::Adt | TableType::Database => adt(
            entry,
            Dest {
                given: dest,
                folder: dest_folder,
                target: &target,
            },
            passwords,
            call,
            copy,
            rows,
        ),
        table_type => dbf(entry, &target, dest_folder, table_type, copy, &path_text),
    };
    match written {
        Ok(true) => {
            copy_index(&entry.path, dest_folder, entry.table_type);
            true
        }
        Ok(false) => false,
        Err(refusal) => {
            refuse(rows, refusal);
            false
        }
    }
}

/// Copies the structural index file of the table `source`, as it is.
fn copy_index(source: &Path, dest_folder: &Path, table_type: TableType) {
    let extension = match table_type {
        TableType::Adt | TableType::Database => "adi",
        TableType::Ntx => return,
        _ => "cdx",
    };
    let Some(folder) = source.parent() else {
        return;
    };
    let stem = source.file_stem().unwrap_or_default().to_string_lossy();
    if let Some(index) = paths::find(folder, &format!("{stem}.{extension}")) {
        let _ = fs::copy(
            &index,
            dest_folder.join(index.file_name().unwrap_or_default()),
        );
    }
}

/// Where the image of a table goes.
struct Dest<'a> {
    given: &'a str,
    folder: &'a Path,
    target: &'a Path,
}

/// The image of an ADT table; whether it was made.
fn adt(
    entry: &Entry,
    dest: Dest,
    passwords: &Passwords,
    call: Option<differential::Call>,
    mut copy: image::Copy,
    rows: &mut Rows,
) -> Result<bool, Refusal> {
    let Dest {
        given: dest,
        folder: dest_folder,
        target,
    } = dest;
    let path_text = paths::join(paths::folder_of(&entry.given), &entry.name);
    let mut password = None;
    if database::image_is_encrypted(&entry.path) {
        match passwords.lookup(&entry.name) {
            Lookup::Given(given) => password = Some(given),
            lookup => {
                let _ = fs::create_dir_all(dest_folder);
                stub(&entry.path, target);
                rows.push(Notice::NoPassword {
                    path: paths::join(dest, &entry.name),
                    listed: matches!(lookup, Lookup::NotListed),
                });
                rows.push(Notice::BackingUp {
                    table: entry.name.clone(),
                    code: 5097,
                });
                return Ok(false);
            }
        }
    }
    copy.password = password
        .as_deref()
        .map(|password| password.as_bytes().to_vec());
    let opened =
        TableFile::open_deciphering(&entry.path, Format::Adt, false, copy.password.as_deref());
    let mut file = match opened {
        Ok(file) => file,
        Err(error) if open_code(&error) == 7060 => {
            let _ = fs::create_dir_all(dest_folder);
            stub(&entry.path, target);
            rows.push(Notice::BackingUp {
                table: entry.name.clone(),
                code: 7060,
            });
            return Ok(false);
        }
        Err(error) => return Err(Refusal::at(open_code(&error), &path_text)),
    };
    fs::create_dir_all(dest_folder).map_err(|_| Refusal::at(7008, &path_text))?;
    if let Some(call) = call {
        // A Diff that only gave the image the changes leaves its index file as
        // the last whole copy had it (`fixtures/backup/s55_free_diff`).
        let whole = differential::table(
            &entry.path,
            target,
            TableType::Adt,
            copy.password.as_deref(),
            call,
            paths::join(dest, &entry.name),
            rows,
        )
        .map_err(|code| Refusal::at(code, &path_text))?;
        return Ok(whole);
    }
    image::write_image(&mut file, &entry.path, target, &copy)
        .map_err(|error| Refusal::at(open_code(&error), &path_text))?;
    Ok(true)
}

/// What an encrypted table leaves in the image when its password is missing
/// or wrong: its header as a free table's image has it, in the clear, and
/// the memo file of a table that has none.
fn stub(source: &Path, target: &Path) {
    let Ok(mut head) = fs::read(source) else {
        return;
    };
    if head.len() < tflip_storage::adt::HEADER_SIZE {
        return;
    }
    let data = u32::from_le_bytes(head[0x20..0x24].try_into().unwrap()) as usize;
    head.truncate(data);
    head[0x18..0x1C].fill(0);
    head[0x28..0x2C].fill(0);
    head[0x30..0x34].fill(0);
    head[0x38..0x4D].fill(0);
    let flags = u32::from_le_bytes(head[0x58..0x5C].try_into().unwrap());
    head[0x58..0x5C]
        .copy_from_slice(&(flags & !tflip_storage::table::flags::ENCRYPTED).to_le_bytes());
    let _ = fs::write(target, &head);
    if let Ok(memo) = fs::read(MemoFile::path_for(source))
        && memo.len() >= 8
    {
        let block_size = u16::from_be_bytes([memo[6], memo[7]]);
        let _ = fs::write(
            MemoFile::path_for(target),
            tflip_storage::create::adm_bytes(block_size),
        );
    }
}

/// The image of a DBF table opened as `table_type`; whether it was made.
fn dbf(
    entry: &Entry,
    target: &Path,
    dest_folder: &Path,
    table_type: TableType,
    mut copy: image::Copy,
    path_text: &str,
) -> Result<bool, Refusal> {
    let wanted = match table_type {
        TableType::Ntx => Flavour::Clipper,
        TableType::Cdx => Flavour::FoxPro,
        TableType::Vfp => Flavour::Visual,
        _ => return Err(Refusal::at(7016, path_text)),
    };
    let corrupt = || Refusal::at(7016, path_text);
    let actual = flavour_of(&entry.path).map_err(|()| corrupt())?;
    let wrong = |is: &str| Refusal {
        code: 5025,
        detail: format!("Should have been ADS_{is}."),
    };
    let opened_as = match (actual, wanted) {
        (Some(Flavour::Clipper), Flavour::FoxPro) => return Err(wrong("NTX")),
        (Some(Flavour::FoxPro), Flavour::Clipper) => return Err(wrong("CDX")),
        (Some(Flavour::Clipper), Flavour::Visual) => return Err(corrupt()),
        (Some(actual), _) => actual,
        (None, wanted) => wanted,
    };
    let mut file = TableFile::open_deciphering(&entry.path, Format::Dbf(opened_as), false, None)
        .map_err(|error| Refusal::at(open_code(&error), path_text))?;
    fs::create_dir_all(dest_folder).map_err(|_| Refusal::at(7008, path_text))?;
    copy.flavour = Some(wanted);
    image::write_image(&mut file, &entry.path, target, &copy)
        .map_err(|error| Refusal::at(open_code(&error), path_text))?;
    Ok(true)
}

/// What a free-table restore is asked for.
pub(crate) struct Restore<'a> {
    pub source: &'a str,
    pub dest: &'a str,
    pub options: &'a Options,
    pub passwords: Option<&'a str>,
}

/// Restores the free tables of the image folder `restore.source` into the
/// folder `restore.dest`: every table's files are copied, and the ADT tables'
/// index files made again.
pub(crate) fn restore(restore: &Restore) -> Result<Rows, Error> {
    let Restore {
        source,
        dest,
        options,
        ..
    } = *restore;
    let mut rows = Rows::default();
    // The archive is taken from the image folder and unpacked to a place that
    // goes away with the call; what it holds is the image.
    let unpacked = database::Unpacked::new();
    let archive = options.archive.as_ref().filter(|a| !a.name.is_empty());
    let image_folder = paths::local(source).filter(|folder| folder.is_dir());
    if let Some(archive) = archive {
        database::unpack(image_folder.as_deref(), source, archive, &unpacked)?;
    }
    let dest_folder = paths::local(dest);
    let made = dest_folder
        .as_deref()
        .is_some_and(|folder| fs::create_dir_all(folder).is_ok());
    let source_folder = match archive {
        Some(_) => Some(unpacked.0.clone()),
        None => image_folder,
    };
    let listed = source_folder.as_deref().map(files_of).unwrap_or_default();
    let tables: Vec<&(String, PathBuf)> = listed
        .iter()
        .filter(|(name, _)| {
            matches!(
                extension_of(name).to_ascii_lowercase().as_str(),
                "adt" | "dbf"
            )
        })
        .collect();
    if tables.is_empty() || options.meta_only {
        rows.push(Notice::NoTablesToRestore {
            path: format!("{}\\", source.trim_end_matches(['\\', '/'])),
        });
        return Ok(rows);
    }
    let passwords = Passwords::parse(restore.passwords);
    let named =
        |list: &[String], name: &str| list.iter().any(|item| item.eq_ignore_ascii_case(name));
    for (name, image_path) in &tables {
        let wanted = options
            .include
            .as_ref()
            .is_none_or(|list| named(list, name))
            && !named(&options.exclude, name);
        if !wanted {
            continue;
        }
        let image_name = paths::join(source, name);
        let (Some(dest_folder), true) = (dest_folder.as_deref(), made) else {
            rows.push(Notice::CopyFree {
                table: image_name,
                code: 3,
            });
            continue;
        };
        let restored = dest_folder.join(name);
        if paths::same(image_path, &restored) {
            rows.push(Notice::CopyFree {
                table: image_name,
                code: 32,
            });
            continue;
        }
        if options.dont_overwrite && paths::find(dest_folder, name).is_some() {
            rows.push(Notice::CopyFree {
                table: image_name,
                code: 80,
            });
            continue;
        }
        if let Err(code) = copy_files(image_path, dest_folder) {
            rows.push(Notice::CopyFree {
                table: image_name,
                code,
            });
            continue;
        }
        if extension_of(name).eq_ignore_ascii_case("adt")
            && paths::find(dest_folder, &format!("{}.adi", stem_of(name))).is_some()
        {
            reindex(dest, dest_folder, name, &passwords, &mut rows);
        } else if let Some(table_type) =
            mapped_type(options.table_type_map.as_deref(), extension_of(name))
            && table_type != TableType::Adt
        {
            // A DBF table the call maps a type to is opened as it, which finds
            // its structural index and makes it again; one it does not is not
            // opened, and the original says nothing of a table that fails to.
            if paths::find(dest_folder, &format!("{}.cdx", stem_of(name))).is_some() {
                let _ = image::mark_structural(&restored);
            }
            let _ = rebuild(dest_folder, name, "", table_type);
        }
    }
    for item in options.include.iter().flatten() {
        if !tables
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(item))
        {
            rows.push(Notice::CopyFree {
                table: paths::join(source, item),
                code: 2,
            });
        }
    }
    Ok(rows)
}

fn stem_of(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// Copies the table file `table` and the memo and index files that go with
/// it into `dest_folder`; the system's code of what failed.
fn copy_files(table: &Path, dest_folder: &Path) -> Result<(), u32> {
    let code = |error: std::io::Error| error.raw_os_error().map_or(5, |code| code as u32);
    let folder = table.parent().unwrap_or(Path::new("."));
    let name = table.file_name().unwrap_or_default().to_string_lossy();
    let stem = stem_of(&name).to_owned();
    fs::copy(table, dest_folder.join(&*name)).map_err(code)?;
    for extension in ["adm", "adi", "fpt", "dbt", "cdx"] {
        if let Some(file) = paths::find(folder, &format!("{stem}.{extension}")) {
            fs::copy(
                &file,
                dest_folder.join(file.file_name().unwrap_or_default()),
            )
            .map_err(code)?;
        }
    }
    Ok(())
}

/// Makes the index files of the restored ADT table `name` again, as a
/// connection of the server's own does.
fn reindex(dest: &str, dest_folder: &Path, name: &str, passwords: &Passwords, rows: &mut Rows) {
    let restored = dest_folder.join(name);
    let named = paths::join(dest, name);
    let mut password = String::new();
    if database::image_is_encrypted(&restored) {
        let detail = match passwords.lookup(name) {
            Lookup::Given(given) => {
                let known = TableFile::open_deciphering(
                    &restored,
                    Format::Adt,
                    false,
                    Some(given.as_bytes()),
                );
                if known.is_err() {
                    rows.push(Notice::Reindex {
                        table: named,
                        code: 7060,
                        detail: None,
                    });
                    return;
                }
                password = given;
                None
            }
            Lookup::Missing => Some("Encrypted table encountered but no passwords were supplied."),
            Lookup::NotListed => {
                Some("Encrypted table encountered but no password was passed in for this table.")
            }
        };
        if let Some(detail) = detail {
            rows.push(Notice::Reindex {
                table: named,
                code: 5097,
                detail: Some(detail.to_owned()),
            });
            return;
        }
    }
    if let Err(error) = rebuild(dest_folder, name, &password, TableType::Adt) {
        rows.push(Notice::Reindex {
            table: named,
            code: error.to_ace().code,
            detail: None,
        });
    }
}

fn rebuild(folder: &Path, name: &str, password: &str, table_type: TableType) -> Result<(), Error> {
    let own = Session::new();
    let handle = connection::connect_path(
        &own,
        &folder.to_string_lossy(),
        connection::LOCAL,
        None,
        None,
    )?;
    let result = crate::table::opening_free_itself(|| {
        let table = crate::table::open(
            &own,
            handle,
            name,
            TableSettings {
                table_type,
                options: crate::table::options::EXCLUSIVE,
                password: password.to_owned(),
                ..TableSettings::default()
            },
        )?;
        let rebuilt = crate::index::reindex(&own, table);
        crate::table::close(&own, table)?;
        rebuilt
    });
    let _ = connection::disconnect(&own, handle);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_match_whatever_the_case() {
        assert!(matches("*.adt", "F_ADT.adt"));
        assert!(matches("F_*.ADT", "f_adt.adt"));
        assert!(matches("f_adt.adt", "F_Adt.ADT"));
        assert!(matches("f_?dt.adt", "f_adt.adt"));
        assert!(!matches("*.dbf", "f_adt.adt"));
        assert!(!matches("f_adt", "f_adt.adt"));
    }

    #[test]
    fn the_map_names_a_type_by_extension() {
        let map = Some("adt=ADS_ADT,dbf=ads_cdx");
        assert_eq!(mapped_type(map, "DBF"), Some(TableType::Cdx));
        assert_eq!(mapped_type(map, "adt"), Some(TableType::Adt));
        assert_eq!(mapped_type(map, "cdx"), None);
        assert_eq!(mapped_type(Some("dbf=ADS_BOGUS"), "dbf"), None);
        assert_eq!(mapped_type(Some("dbf"), "dbf"), None);
        assert_eq!(mapped_type(Some("dbf=2"), "dbf"), None);
    }

    #[test]
    fn passwords_are_one_or_listed_by_file_name() {
        assert!(matches!(
            Passwords::parse(Some("")).lookup("a.adt"),
            Lookup::Missing
        ));
        assert!(matches!(
            Passwords::parse(Some("pw")).lookup("a.adt"),
            Lookup::Given(password) if password == "pw"
        ));
        let listed = Passwords::parse(Some("A.ADT=one;b.adt=two"));
        assert!(matches!(listed.lookup("a.adt"), Lookup::Given(password) if password == "one"));
        assert!(matches!(listed.lookup("c.adt"), Lookup::NotListed));
        assert!(matches!(listed.lookup("a"), Lookup::NotListed));
    }
}
