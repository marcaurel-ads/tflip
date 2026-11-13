//! `sp_BackupDatabase` and `sp_RestoreDatabase`: the image of a dictionary
//! and its tables in a folder, and the dictionary made from one.

use std::fs;
use std::path::{Path, PathBuf};

use tflip_storage::archive;
use tflip_storage::dbf::Flavour;
use tflip_storage::dd::{self, Dictionary, Kind};
use tflip_storage::image;
use tflip_storage::memo::MemoKind;
use tflip_storage::table::{Format, TableFile};

use super::active;
use super::differential::{self, Mode};
use super::options::{Archive, Options, table_name_of};
use super::paths;
use super::rows::{Notice, Rows};
use crate::handle::Handle;
use crate::table::TableType;
use crate::{Error, Session, ace_error, connection, shared};

/// A table of the dictionary being backed up.
struct Listed {
    name: String,
    path: PathBuf,
    table_type: TableType,
}

/// The format of a table file of a dictionary's table of `table_type`.
pub(crate) fn format_of(table_type: TableType) -> Format {
    match table_type {
        TableType::Adt | TableType::Database => Format::Adt,
        TableType::Ntx => Format::Dbf(Flavour::Clipper),
        TableType::Cdx => Format::Dbf(Flavour::FoxPro),
        TableType::Vfp => Format::Dbf(Flavour::Visual),
    }
}

/// The memo file of the table file `table` of `table_type`, named as the
/// original names it.
pub(crate) fn memo_path(table: &Path, table_type: TableType) -> PathBuf {
    match table_type {
        TableType::Ntx => tflip_storage::dbf::memo_path(table, MemoKind::Dbt),
        TableType::Cdx | TableType::Vfp => tflip_storage::dbf::memo_path(table, MemoKind::Fpt),
        _ => tflip_storage::memo::MemoFile::path_for(table),
    }
}

fn file_creation() -> Error {
    ace_error::sql_bare(7040)
}

/// The code a row gives for a table file that does not open as it should.
pub(super) fn open_code(error: &tflip_storage::table::Error) -> u32 {
    use tflip_storage::table::Error as E;
    match error {
        E::Io(error) if error.kind() == std::io::ErrorKind::NotFound => 7041,
        E::Io(_) => 7008,
        E::Encrypted | E::EncryptedDescriptors => 7060,
        _ => 7016,
    }
}

/// Backs up the dictionary `connection` is to into the folder `dest` (as the
/// call gave it): its files, and the image of each table that `options` ask
/// for. The rows are what the original's result set has.
pub(crate) fn backup(
    session: &Session,
    connection: Handle,
    dest: &str,
    options: &Options,
) -> Result<Rows, Error> {
    let owner = connection::get(session, connection)?;
    if !paths::is_absolute(dest) {
        return Err(ace_error::sql(5020, "invalid destination path"));
    }
    let (tables, password) = owner
        .with_database_opt(|db| {
            let mut listed: Vec<Listed> = db
                .table_names()
                .iter()
                .filter_map(|name| {
                    let bound = db.table(name)?;
                    Some(Listed {
                        name: name.clone(),
                        path: bound.path,
                        table_type: bound.table_type,
                    })
                })
                .collect();
            listed.sort_by_key(|table| table.name.to_ascii_lowercase());
            let password = db.dictionary().table_password(db.password());
            (listed, password)
        })
        .ok_or(Error::InvalidConnection)?;
    // Names that are no table: all of them is a refusal, some of them a row.
    let unknown: Vec<&String> = options
        .include
        .iter()
        .flatten()
        .filter(|item| {
            !tables
                .iter()
                .any(|table| table.name.eq_ignore_ascii_case(table_name_of(item)))
        })
        .collect();
    if options
        .include
        .as_ref()
        .is_some_and(|list| !list.is_empty())
        && unknown.len() == options.include.as_ref().map_or(0, Vec::len)
    {
        return Err(ace_error::sql_bare(5132));
    }
    let folder = paths::local(dest).ok_or_else(file_creation)?;
    // A dictionary file given as the destination is connected to, which makes
    // its index, before the call fails.
    if folder.is_file() && has_add_extension(dest) && !folder.with_extension("ai").exists() {
        let _ = crate::dictionary_write::rebuild_index(&Session::new(), &folder);
    }
    let add = owner.path.clone();
    let add_name = add.file_name().ok_or_else(file_creation)?;
    fs::create_dir_all(&folder).map_err(|_| file_creation())?;
    let image_add = folder.join(add_name);
    if image_add.exists() && paths::same(&image_add, &add) || image_add.is_dir() {
        return Err(file_creation());
    }
    // The files this backup takes are its own until it ends: a second backup
    // meets them as 7131 rows, and no connection opens a table meanwhile.
    let wanted: Vec<&Listed> = tables
        .iter()
        .filter(|table| !options.meta_only && options.wants(&table.name))
        .collect();
    let dictionary_hold = active::Hold::take(&add);
    let holds: Vec<Option<active::Hold>> = wanted
        .iter()
        .map(|table| active::Hold::take(&table.path))
        .collect();
    if dictionary_hold.is_some() {
        active::paused(&add);
    }
    let mut rows = Rows::default();
    let stamp = differential::stamp();
    let mode = differential::mode_of(options);
    // What the backup wrote, in image order, for an archive.
    let mut written: Vec<PathBuf> = Vec::new();
    if dictionary_hold.is_some() {
        // A `Diff` writes the dictionary's files itself, into the image it
        // finds.
        if mode != Some(Mode::Diff) {
            fs::copy(&add, &image_add).map_err(|_| file_creation())?;
            let memo = dd::memo_path(&add);
            if memo.exists() {
                fs::copy(&memo, dd::memo_path(&image_add)).map_err(|_| file_creation())?;
            }
        }
        // A full image is no base of a differential backup, whatever stamp
        // the source has.
        if mode.is_none() && image::read_stamp(&image_add).is_ok_and(|stamp| stamp != 0) {
            let _ = image::write_stamp(&image_add, 0);
        }
        if let Some(mode) = mode {
            let row_name = paths::join(dest, &add_name.to_string_lossy());
            differential::dictionary(&add, &image_add, row_name, mode, stamp, &mut rows)?;
        }
        written.push(image_add.clone());
        let image_memo = dd::memo_path(&image_add);
        if image_memo.exists() {
            written.push(image_memo);
        }
    }
    if !options.meta_only {
        for name in unknown {
            rows.push(Notice::UnknownTable { name: name.clone() });
        }
    }
    for (table, hold) in wanted.into_iter().zip(&holds) {
        if hold.is_none() {
            rows.push(Notice::InAnotherBackup {
                table: table.name.clone(),
                dictionary: false,
            });
            continue;
        }
        let given = options
            .include
            .iter()
            .flatten()
            .find(|item| table_name_of(item).eq_ignore_ascii_case(&table.name));
        let stem = match given {
            Some(item) => table_name_of(item).to_owned(),
            None => table
                .path
                .file_stem()
                .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned()),
        };
        let extension = table
            .path
            .extension()
            .map_or_else(String::new, |e| e.to_string_lossy().into_owned());
        let target = folder.join(format!("{stem}.{extension}"));
        // A table another connection holds exclusively is not copied; the
        // others are.
        let copied = match shared::held_exclusively(&table.path) {
            true => Err(7008),
            false => match mode {
                Some(mode) => differential::table(
                    &table.path,
                    &target,
                    table.table_type,
                    password.as_deref(),
                    differential::Call { mode, stamp },
                    paths::join(dest, &stem),
                    &mut rows,
                )
                .map(drop),
                None => copy_table(&table.path, &target, table.table_type, password.as_deref()),
            },
        };
        match copied {
            Ok(()) => {
                written.push(target.clone());
                let memo = memo_path(&target, table.table_type);
                if memo.exists() {
                    written.push(memo);
                }
            }
            Err(code) => rows.push(Notice::OpenForBackup {
                table: table.name.clone(),
                code,
                detail: None,
                free: false,
            }),
        }
    }
    if dictionary_hold.is_none() {
        rows.push(Notice::InAnotherBackup {
            table: add.display().to_string().replace('/', "\\"),
            dictionary: true,
        });
    }
    if let Some(archive) = &options.archive {
        archive_image(&folder, dest, archive, &written, &mut rows);
    }
    Ok(rows)
}

/// Packs the files `written` of the image in `folder` into the archive the
/// options name, and removes them once it is made. An archive that cannot be
/// made leaves them (as the original does) and is told in the rows.
pub(crate) fn archive_image(
    folder: &Path,
    dest: &str,
    archive: &Archive,
    written: &[PathBuf],
    rows: &mut Rows,
) {
    let cannot = |rows: &mut Rows| {
        // The original fails twice for a name it cannot create, from two
        // places.
        for code in [3, 7040] {
            rows.push(Notice::ArchiveFile {
                dest: dest.to_owned(),
                code,
            });
        }
        rows.push(Notice::ArchiveBackup { table: None });
    };
    if archive.name.is_empty() {
        return cannot(rows);
    }
    let path = folder.join(archive.name.replace('\\', "/"));
    if let Some(parent) = path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return cannot(rows);
    }
    let names: Vec<String> = written.iter().map(|file| file_name_of(file)).collect();
    let members: Vec<(&str, archive::Source)> = names
        .iter()
        .zip(written)
        .map(|(name, file)| (name.as_str(), archive::Source::File(file)))
        .collect();
    // One time for every member: the backup's.
    let mtime = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    match archive::write_archive(&path, &members, mtime, archive.compressed) {
        Ok(()) => {
            for file in written {
                let _ = fs::remove_file(file);
            }
        }
        Err(archive::ArchiveError::NameTooLong(name)) => {
            rows.push(Notice::ArchiveNameTooLong {
                path: paths::join(dest, &name),
            });
            rows.push(Notice::ArchiveBackup {
                table: Some(archive.name.clone()),
            });
        }
        Err(_) => cannot(rows),
    }
}

/// Unpacks the archive `archive` of the image folder `folder` (as the call
/// gave it, `given`) into `unpacked`.
pub(super) fn unpack(
    folder: Option<&Path>,
    given: &str,
    archive: &Archive,
    unpacked: &Unpacked,
) -> Result<(), Error> {
    let path = folder
        .unwrap_or(Path::new("."))
        .join(archive.name.replace('\\', "/"));
    match archive::extract_archive(&path, archive.compressed, &unpacked.0, |_| true) {
        Ok(_) => Ok(()),
        Err(archive::ArchiveError::Io(_)) => Err(ace_error::sql(
            7008,
            &format!(
                "Error opening archive file {}.",
                paths::join(given, &archive.name)
            ),
        )),
        Err(_) => Err(ace_error::sql(
            5198,
            "The checksum for an archive entry is not correct. The archive file is not valid.",
        )),
    }
}

/// Whether the path as the call gave it ends in `.add`.
fn has_add_extension(path: &str) -> bool {
    path.rsplit(['\\', '/'])
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("add"))
}

/// Writes the image of the table file `source`, of `table_type`, to `image`,
/// and its memo file next to it; the code of the error otherwise. `password`
/// is the one the table's records and descriptors are enciphered with.
///
/// This is the one function that makes a table's image: the free-table and
/// differential backups call it with the table type and `copy` they need.
pub(crate) fn copy_table(
    source: &Path,
    target: &Path,
    table_type: TableType,
    password: Option<&[u8]>,
) -> Result<(), u32> {
    let copy = image::Copy {
        password: password.map(<[u8]>::to_vec),
        overrides: shared::committed_view(source),
        ..image::Copy::default()
    };
    copy_table_with(source, target, table_type, &copy)
}

/// [`copy_table`] with what the image keeps of the table asked for (`copy`,
/// whose password also opens an encrypted table).
pub(crate) fn copy_table_with(
    source: &Path,
    target: &Path,
    table_type: TableType,
    copy: &image::Copy,
) -> Result<(), u32> {
    let format = format_of(table_type);
    let mut file = TableFile::open_deciphering(source, format, false, copy.password.as_deref())
        .map_err(|error| open_code(&error))?;
    image::write_image(&mut file, source, target, copy).map_err(|error| open_code(&error))
}

/// Whether a connection of any session of the server has the dictionary at
/// `add` open.
fn dictionary_open(session: &Session, add: &Path) -> bool {
    let mut sessions = Session::served();
    sessions.push(session.clone());
    sessions.iter().any(|session| {
        connection::all(session)
            .iter()
            .any(|open| open.dictionary && paths::same(&open.path, add))
    })
}

/// What a restore is asked for.
pub(crate) struct Restore<'a> {
    /// The image's dictionary and the dictionary to make, as the call gave
    /// them.
    pub source: &'a str,
    pub dest: &'a str,
    /// The password of the user the restore connects as.
    pub password: Option<&'a str>,
    pub options: &'a Options,
}

/// Restores the image `restore.source` names into the dictionary
/// `restore.dest`: the dictionary's files, then the tables its objects name,
/// each copied from the image with its index files made. Nothing is looked at
/// beyond the paths and the options before the call: what happens after is
/// in the rows.
pub(crate) fn restore(session: &Session, restore: &Restore) -> Result<Rows, Error> {
    let Restore {
        source,
        dest,
        password,
        options,
    } = *restore;
    let invalid = |text| ace_error::sql(5020, text);
    let has_add = has_add_extension;
    // The archive replaces the image's files, which are not in the folder.
    let archive = options.archive.as_ref().filter(|a| !a.name.is_empty());
    let given_add = paths::local(source)
        .filter(|path| {
            paths::is_absolute(source) && has_add(source) && (archive.is_some() || path.is_file())
        })
        .ok_or_else(|| invalid("Source path must include exisiting dictionary file name."))?;
    if !paths::is_absolute(dest) || !has_add(dest) {
        return Err(invalid(
            "Destination path must include existing dictionary file name.",
        ));
    }
    // The archive is taken from the image's folder and unpacked to a place
    // that goes away with the call.
    let unpacked = Unpacked::new();
    let source_add = match archive {
        Some(archive) => {
            unpack(
                given_add.parent(),
                paths::folder_of(source),
                archive,
                &unpacked,
            )?;
            unpacked.0.join(given_add.file_name().unwrap_or_default())
        }
        None => given_add.clone(),
    };
    let mut rows = Rows::default();
    let dest_add = paths::local(dest).filter(|add| {
        add.parent()
            .is_some_and(|folder| fs::create_dir_all(folder).is_ok())
    });
    let Some(dest_add) = dest_add else {
        rows.push(Notice::DictionaryMetadata {
            source: source.to_owned(),
            code: 3,
        });
        return Ok(rows);
    };
    if dictionary_open(session, &dest_add) || paths::same(&given_add, &dest_add) {
        rows.push(Notice::DictionaryMetadata {
            source: source.to_owned(),
            code: 32,
        });
        return Ok(rows);
    }
    let copied = fs::copy(&source_add, &dest_add).and_then(|_| {
        let memo = dd::memo_path(&source_add);
        match memo.exists() {
            true => fs::copy(&memo, dd::memo_path(&dest_add)).map(drop),
            false => Ok(()),
        }
    });
    if copied.is_err() {
        rows.push(Notice::DictionaryMetadata {
            source: source.to_owned(),
            code: 32,
        });
        return Ok(rows);
    }
    if options.meta_only {
        return Ok(rows);
    }
    if options.dd_password.is_some() {
        return Err(ace_error::sql_bare(7160));
    }
    // The dictionaries get their index when a connection is made to them.
    let own = Session::new();
    let _ = fs::remove_file(dest_add.with_extension("ai"));
    crate::dictionary_write::rebuild_index(&own, &dest_add)?;
    if !source_add.with_extension("ai").exists() {
        crate::dictionary_write::rebuild_index(&own, &source_add)?;
    }
    login(
        &dest_add,
        options.user.as_deref(),
        password.unwrap_or_default(),
    )?;
    let admin_password = password.unwrap_or_default();
    tables(
        &own,
        source,
        &source_add,
        &dest_add,
        admin_password,
        options,
        &mut rows,
    )?;
    Ok(rows)
}

/// Connects as the user the restore acts for, or as the administrator: the
/// refusals are the original's.
fn login(add: &Path, user: Option<&str>, password: &str) -> Result<(), Error> {
    let mut database =
        crate::dictionary::Database::open(add).map_err(|_| ace_error::sql_bare(5100))?;
    let name = user.unwrap_or("ADSSYS");
    let key = match database.login(Some(name), password.as_bytes()) {
        Ok(key) => key,
        Err(_) if name.eq_ignore_ascii_case("ADSSYS") || !database.has_user(name) => {
            return Err(ace_error::sql_bare(5100));
        }
        Err(_) => return Err(ace_error::sql_bare(7078)),
    };
    database.set_identity(Some(name), key);
    match database.may_back_up() {
        true => Ok(()),
        false => Err(ace_error::sql_bare(5054)),
    }
}

/// Restores the tables of the image's dictionary.
fn tables(
    own: &Session,
    source: &str,
    source_add: &Path,
    dest_add: &Path,
    admin_password: &str,
    options: &Options,
    rows: &mut Rows,
) -> Result<(), Error> {
    let dictionary = Dictionary::open(dest_add).map_err(|_| Error::CorruptTable)?;
    // The password the dictionary's encrypted tables share, which the
    // administrator's releases.
    let table_password = dictionary
        .table_password(admin_password.as_bytes())
        .filter(|password| !password.is_empty());
    let indexes_encrypted = dictionary
        .database()
        .is_some_and(|database| database.has(dd::database::ENCRYPT_INDEXES));
    let mut listed = dictionary.all(Kind::Table);
    listed.sort_by_key(|table| table.name.to_ascii_lowercase());
    let given: Vec<&String> = options.include.iter().flatten().collect();
    let unknown: Vec<&&String> = given
        .iter()
        .filter(|item| {
            !listed
                .iter()
                .any(|table| table.name.eq_ignore_ascii_case(table_name_of(item)))
        })
        .collect();
    if !given.is_empty() && unknown.len() == given.len() {
        return Err(ace_error::sql_bare(5132));
    }
    let image_folder = source_add.parent().unwrap_or(Path::new("."));
    let dest_folder = dest_add.parent().unwrap_or(Path::new("."));
    let image_given = paths::folder_of(source);
    let dictionary_name = dest_add
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    for item in unknown {
        let stem = table_name_of(item);
        rows.push(Notice::OpenSource {
            table: paths::join(image_given, stem),
            code: 7041,
            detail: paths::join(image_given, &format!("{stem}.adt")),
        });
    }
    for table in listed.iter().filter(|table| options.wants(&table.name)) {
        let relative = table.text(dd::table::RELATIVE_PATH).unwrap_or_default();
        let table_type = crate::dictionary::table_type(table);
        let dest_table = crate::dictionary_write::under_folder(dest_folder, &relative);
        let file_name = relative.rsplit(['\\', '/']).next().unwrap_or(&relative);
        let stem = file_name
            .rsplit_once('.')
            .map_or(file_name, |(stem, _)| stem);
        let Some(image_table) = paths::find(image_folder, file_name) else {
            rows.push(Notice::OpenSource {
                table: paths::join(image_given, stem),
                code: 7041,
                detail: paths::join(image_given, file_name),
            });
            continue;
        };
        if paths::same(&image_table, &dest_table) || shared::in_use(&dest_table) {
            rows.push(Notice::OpenOrCreate {
                table: table.name.clone(),
                code: 7008,
            });
            continue;
        }
        // Index files that are there are of the tables that were: they are
        // made again with the table.
        for file in dictionary.children(table.id, Kind::IndexFile) {
            let index = file.text(dd::index_file::RELATIVE_PATH).unwrap_or_default();
            let _ = fs::remove_file(crate::dictionary_write::under_folder(dest_folder, &index));
        }
        if let Some(parent) = dest_table.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let copied = fs::copy(&image_table, &dest_table).and_then(|_| {
            let image_memo = memo_path(&image_table, table_type);
            match paths::find(image_folder, &file_name_of(&image_memo)) {
                Some(memo) => fs::copy(memo, memo_path(&dest_table, table_type)).map(drop),
                None => Ok(()),
            }
        });
        if copied.is_err() {
            rows.push(Notice::OpenOrCreate {
                table: table.name.clone(),
                code: 7008,
            });
            continue;
        }
        // An encrypted table's image has its descriptors in the clear and is
        // opened with the table password for its index files; the descriptors
        // are enciphered again once those are made.
        let key = table_password
            .as_deref()
            .filter(|_| table_type == TableType::Adt && image_is_encrypted(&dest_table));
        if table_type == TableType::Adt
            && let Err(code) = differential::unpack(&dest_table, key)
        {
            rows.push(Notice::OpenOrCreate {
                table: table.name.clone(),
                code,
            });
            continue;
        }
        let password = key.map(|key| String::from_utf8_lossy(key).into_owned());
        crate::dictionary_write::build_restored_orders(
            own,
            &dictionary,
            table.id,
            dest_folder,
            &relative,
            table_type,
            password.as_deref().unwrap_or_default(),
        )?;
        if let Some(key) = key {
            if indexes_encrypted {
                for file in dictionary.children(table.id, Kind::IndexFile) {
                    let index = file.text(dd::index_file::RELATIVE_PATH).unwrap_or_default();
                    let path = crate::dictionary_write::under_folder(dest_folder, &index);
                    tflip_storage::adi::encipher_rc4(&path, &key[..key.len().min(20)])
                        .map_err(|_| Error::CorruptIndex)?;
                }
            }
            image::rc4_descriptors(&dest_table, key, true).map_err(|_| Error::CorruptTable)?;
        }
        if table_type == TableType::Adt {
            dd::create::bind_table(&dest_table, table.id, &dictionary_name)
                .map_err(|_| Error::CorruptTable)?;
        }
    }
    Ok(())
}

/// Whether the ADT table file at `path` is RC4-encrypted: its header says so
/// and it is not an AES table.
pub(super) fn image_is_encrypted(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; tflip_storage::adt::HEADER_SIZE];
    let read = fs::File::open(path).and_then(|mut file| file.read_exact(&mut head));
    let flags = u32::from_le_bytes(head[0x58..0x5C].try_into().unwrap());
    read.is_ok()
        && flags & tflip_storage::table::flags::ENCRYPTED != 0
        && matches!(
            head[0x4C],
            tflip_storage::adt::ENCRYPTION_RC4 | tflip_storage::adt::ENCRYPTION_RC4_40
        )
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The folder an archive is unpacked into, removed when the restore ends.
pub(super) struct Unpacked(pub(super) PathBuf);

impl Unpacked {
    pub(super) fn new() -> Unpacked {
        static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!("tflip-restore-{}-{n}", std::process::id());
        Unpacked(std::env::temp_dir().join(name))
    }
}

impl Drop for Unpacked {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
