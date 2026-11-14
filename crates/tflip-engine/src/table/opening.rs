//! Opening and closing tables: by name on a connection, as built cursors,
//! clones of open tables, and deleting their files.

use super::*;
use tflip_storage::crypto::Scheme;

/// The engine error for a table whose structure could not be read.
pub(super) fn structure_error(error: StorageError) -> Error {
    match error {
        StorageError::Io(_) => Error::FileNotFound,
        StorageError::Corrupt(tflip_storage::dbf::BAD_DATE) => Error::FullText(7016),
        StorageError::Corrupt(_) => Error::CorruptTable,
        StorageError::EncryptedDescriptors | StorageError::Encrypted => Error::TableEncrypted,
        StorageError::ReadOnly => Error::TableReadOnly,
        StorageError::NotWritable => Error::NotImplemented,
        StorageError::CorruptMemo => Error::CorruptMemo,
        StorageError::WrongMemoFile(flavour) => Error::WrongTableType(flavour.into()),
    }
}

/// The refusal to open the temporary table `name`, made as `made`, as `asked`:
/// an ADT table, a Visual FoxPro one and the Clipper and FoxPro ones are of
/// three kinds, and one opened as another kind is corrupt (7016); a Clipper
/// table opened as FoxPro and the other way round has an invalid open mode
/// (7028). `Ok` for the type it was made as.
pub(crate) fn temp_type_mismatch(
    name: &str,
    made: TableType,
    asked: TableType,
    pinned: bool,
) -> Result<(), Error> {
    use TableType::{Cdx, Ntx};
    let code = match (made, asked) {
        (made, asked) if made == asked => return Ok(()),
        (Ntx, Cdx) | (Cdx, Ntx) if !pinned => return Ok(()),
        (Ntx, Cdx) | (Cdx, Ntx) => 7028,
        _ => 7016,
    };
    Err(Error::FullText(code).detailed(crate::temp::bare(name)))
}

/// What the original engine adds to the text of 5159, a dictionary's table
/// opened as a free table.
pub(crate) const DATABASE_TABLE: &str =
    "The table is a database table. It cannot be opened as a free table.";

/// Whether the file at `path` is an ADT table of an AES dictionary: its
/// header names the dictionary and its structure is enciphered.
fn aes_dictionary_table(path: &std::path::Path) -> bool {
    use std::io::Read;
    use tflip_storage::adt;
    let mut bytes = [0; adt::HEADER_SIZE];
    let read = std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut bytes));
    match read.ok().and_then(|()| adt::parse_header(&bytes).ok()) {
        Some(header) => {
            header.dictionary
                && header.encrypted_descriptors()
                && matches!(
                    header.encryption_type,
                    adt::ENCRYPTION_AES128 | adt::ENCRYPTION_AES256
                )
        }
        None => false,
    }
}

/// Whether the file at `path` is an ADT table of a dictionary that encrypts
/// its tables' structure with RC4: its header names the dictionary, and its
/// field descriptors are encrypted.
fn rc4_encrypted_structure(path: &std::path::Path) -> bool {
    use std::io::Read;
    use tflip_storage::adt;
    let mut bytes = [0; adt::HEADER_SIZE];
    let read = std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut bytes));
    match read.ok().and_then(|()| adt::parse_header(&bytes).ok()) {
        Some(header) => {
            header.dictionary
                && header.encrypted_descriptors()
                && !matches!(
                    header.encryption_type,
                    adt::ENCRYPTION_AES128 | adt::ENCRYPTION_AES256
                )
        }
        None => false,
    }
}

/// The object number of its dictionary an ADT table's header keeps (0x160;
/// 0 for a free table), if the file can be read.
fn stored_object_id(path: &std::path::Path) -> Option<u32> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let mut bytes = [0; 4];
    file.seek(SeekFrom::Start(0x160)).ok()?;
    file.read_exact(&mut bytes).ok()?;
    Some(u32::from_le_bytes(bytes))
}

/// Whether the file at `path` is an ADT table whose header names a
/// dictionary and whose field descriptors are in the clear (an encrypted
/// structure is refused with its own error when the table is read).
fn names_dictionary(path: &std::path::Path) -> bool {
    use std::io::Read;
    use tflip_storage::adt;
    let mut bytes = [0; adt::HEADER_SIZE];
    let read = std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut bytes));
    match read.ok().and_then(|()| adt::parse_header(&bytes).ok()) {
        Some(header) => header.dictionary && !header.encrypted_descriptors(),
        None => false,
    }
}

/// The open options as the original engine reports them
/// (`AdsGetTableOpenOptions`) for `given`, or why it refuses them. As the
/// original engine does it:
///
/// - The permissions must include reading; none means all four.
///   `READONLY`, or the read permission alone, is read-only: `READONLY`
///   with only the read permission.
/// - `EXCLUSIVE` is kept; with `SHARED` it is [`Error::InvalidOption`].
/// - `FREE_TABLE` and the bits 0x400, 0x800 and 0x8000 are
///   [`Error::InvalidOption`], `TEMP_TABLE` [`Error::TempTableRefused`].
///   Other bits (`SHARED`, Clipper memos, the collation and caching
///   options) are accepted and not reported.
/// - The report always has [`options::OPENED`] set.
/// - The collation mismatch options are not reported; both is
///   [`Error::InvalidOption`].
pub fn open_options(given: u32) -> Result<u32, Error> {
    use options::*;
    const REFUSED: u32 = 0x0400 | 0x0800 | FREE_TABLE | 0x8000;
    const PERMISSIONS: u32 = PERM_READ | PERM_UPDATE | PERM_INSERT | PERM_DELETE;
    if given & TEMP_TABLE != 0 && given & REFUSED == 0 {
        return Err(Error::TempTableRefused);
    }
    let both = EXCLUSIVE | SHARED;
    let permissions = match given & PERMISSIONS {
        0 => PERMISSIONS,
        permissions => permissions,
    };
    if given & both == both {
        return Err(Error::InvalidOption
            .detailed("Parameter 8 (ADS_SHARED and ADS_EXCLUSIVE cannot be used together)"));
    }
    if given & (REINDEX_ON_MISMATCH | IGNORE_MISMATCH) == REINDEX_ON_MISMATCH | IGNORE_MISMATCH {
        return Err(Error::InvalidOption.detailed(
            "Parameter 8 - cannot specify both ADS_REINDEX_ON_COLLATION_MISMATCH and \
             ADS_IGNORE_COLLATION_MISMATCH",
        ));
    }
    if given & REFUSED != 0 {
        return Err(Error::InvalidOption.detailed("Parameter 8"));
    }
    if permissions & PERM_READ == 0 {
        return Err(Error::InvalidOption);
    }
    let read_only = given & READONLY != 0 || permissions == PERM_READ;
    let access = match read_only {
        true => READONLY | PERM_READ,
        false => permissions,
    };
    Ok(OPENED | given & EXCLUSIVE | access)
}

/// The `char_type` of ANSI tables (`ADS_ANSI`).
pub(super) const ANSI: u16 = 1;

/// The `char_type` of OEM tables (`ADS_OEM`).
pub(super) const OEM: u16 = 2;

/// The character type of a Visual FoxPro table opened with `char_type` and
/// `collation`, as the original engine reports it and converts its text
/// (`fixtures/dbf/vfp_oem.json`): `ADS_OEM` for `ADS_OEM`, the collation `OEM`
/// and the collations of an OEM code page (the char types 25 to 68 and 92
/// to 118 of `ace.h`, or such a name, `GENERAL_VFP_CI_AS_437`); else
/// `ADS_ANSI`. A collation's name stands for the character type.
fn vfp_char_type(char_type: u16, collation: &str) -> u16 {
    let collation = collation.trim();
    if collation.is_empty() {
        return match char_type {
            OEM | 25..=68 | 92..=118 => OEM,
            _ => ANSI,
        };
    }
    let oem_page = collation
        .rsplit_once('_')
        .and_then(|(_, page)| page.parse::<u32>().ok())
        .is_some_and(|page| !(1250..=1258).contains(&page));
    match collation.eq_ignore_ascii_case("OEM") || oem_page {
        true => OEM,
        false => ANSI,
    }
}

/// Opens the table `name` on `connection` with `settings`.
///
/// The options are checked and reported as [`open_options`] says. ADT
/// tables are ANSI whatever they are opened as, as in the original engine;
/// a Visual FoxPro table is OEM as [`vfp_char_type`] says (a dictionary's
/// if the dictionary says so).
///
/// On a connection to a data dictionary, `name` is a table of the
/// dictionary, by its name or its file, which opens with the dictionary's
/// index files for it; or a view, which opens as the cursor of its
/// `SELECT`; or else a free table in the dictionary's folder, and if there
/// is none [`Error::InvalidObjectName`].
///
/// A relative `name` is taken relative to the connection's path. Connection 0
/// opens the table on its own, with `name` taken as is. An ADT table's name
/// may leave out `.adt`, as in the original engine. The table is owned by
/// its connection: disconnecting closes it.
///
/// The table's structure is read now: a file that cannot be opened is
/// [`Error::FileNotFound`], one that is not a valid table of its type
/// [`Error::CorruptTable`], and one with encrypted field descriptors
/// [`Error::TableEncrypted`]. A DBF table's name may leave out `.dbf`.
///
/// [`open_for_client`] opens a table a client asks for.
pub fn open(
    session: &Session,
    connection: Handle,
    name: &str,
    settings: TableSettings,
) -> Result<Handle, Error> {
    open_as(session, connection, name, settings, false)
}

/// The error of a dictionary's table one of whose index files is not there.
pub(crate) const MISSING_INDEX_FILE: u32 = 7041;

/// The error of a table that carries another object number than its
/// dictionary has for it.
pub(crate) const ID_MISMATCH: u32 = 7074;

thread_local! {
    /// Whether the table being opened is one a restructure is rebuilding the
    /// index files of, which are not all there yet.
    static REBUILDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

thread_local! {
    /// Whether the engine itself is opening a dictionary's table as a free
    /// one, to make the files its dictionary lacks.
    static OWN_FREE_OPEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `run`, in which the engine may open a dictionary's table as a free
/// one (a client may not).
pub(crate) fn opening_free_itself<T>(run: impl FnOnce() -> T) -> T {
    let before = OWN_FREE_OPEN.replace(true);
    let result = run();
    OWN_FREE_OPEN.set(before);
    result
}

thread_local! {
    /// The table an `INSERT`, `UPDATE` or `DELETE` is changing and the right
    /// the change needs: a user holding it opens that table without the read
    /// right (`fixtures/sql/ddinsertonly`).
    static CHANGING: std::cell::RefCell<Option<(String, u32)>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs `run` with the table `name` open to a user who holds `right` on it
/// without the read right.
pub(crate) fn changing<T>(name: &str, right: u32, run: impl FnOnce() -> T) -> T {
    let before = CHANGING.replace(Some((name.to_owned(), right)));
    let result = run();
    CHANGING.set(before);
    result
}

/// [`open`] of a table whose index files a restructure has yet to rebuild.
pub fn open_rebuilding(
    session: &Session,
    connection: Handle,
    name: &str,
    settings: TableSettings,
) -> Result<Handle, Error> {
    REBUILDING.set(true);
    let opened = open(session, connection, name, settings);
    REBUILDING.set(false);
    opened
}

/// [`open`], for a client (`direct`) or for the engine's own statements.
fn open_as(
    session: &Session,
    connection: Handle,
    name: &str,
    settings: TableSettings,
    direct: bool,
) -> Result<Handle, Error> {
    let mut settings = settings;
    // What to do with an index of another collation (and the locking
    // `AdsOpenTable101` asks for) stays with the settings, out of what
    // `AdsGetTableOpenOptions` reports.
    let kept = settings.options & options::KEPT;
    settings.options = open_options(settings.options)? | kept;
    settings.char_type = match settings.table_type {
        TableType::Adt => ANSI,
        TableType::Vfp => vfp_char_type(settings.char_type, &settings.collation),
        _ => settings.char_type,
    };
    if connection != 0 {
        let owner = connection::get(session, connection)?;
        // A table opened without a collation takes its connection's.
        if settings.collation.is_empty() {
            settings.collation = owner.collation();
        }
        if owner
            .with_database_opt(|database| database.named_public_slot_bad(name))
            .unwrap_or(false)
        {
            return Err(Error::AgentFailed);
        }
        if let Some(sql) = owner
            .with_database_opt(|database| database.view(name))
            .flatten()
        {
            return crate::statement::open_view(session, connection, &sql, &settings.alias);
        }
    }
    let given = (!settings.alias.is_empty()).then(|| settings.alias.clone());
    // A dictionary's `OPEN_TABLE` triggers may cancel the open.
    crate::trigger::fire_open(session, connection, name, &settings, direct, 1)?;
    let mut table = load(session, connection, name, settings.clone())?;
    // A table of permission level 3 is for SQL alone.
    if direct && table.sql_only {
        return Err(Error::FullText(7086));
    }
    let opened = crate::trigger::fire_open(session, connection, name, &settings, direct, 4)?;
    // The alias given, else a dictionary's name for the table, else the
    // name's.
    if let Some(alias) = given.or_else(|| table.alias.get().cloned()) {
        table.alias = OnceLock::from(alias);
    } else if crate::aep::is_procedure_table(name) {
        let _ = table.alias.set(String::new());
    } else {
        let _ = table.alias.set(stem(name));
    }
    table.opened = AtomicBool::new(true);
    // As in the original engine: orders with a dynamic collation need it
    // asked for.
    // Opening anyway is `ADS_IGNORE_COLLATION_MISMATCH`'s; rebuilding the
    // index does not change a dynamic collation (`fixtures/collation/mismatch`).
    for collation in index::index_collations(&table.path) {
        // `ansi` is no dynamic collation.
        let own = table.settings.collation.trim();
        let own = if own.eq_ignore_ascii_case("ansi") {
            ""
        } else {
            own
        };
        if !collation.eq_ignore_ascii_case(own)
            && table.settings.options & options::IGNORE_MISMATCH == 0
        {
            let name = |name: &str| {
                if name.is_empty() {
                    "(none)".to_owned()
                } else {
                    name.to_owned()
                }
            };
            return Err(Error::CollationMismatch.detailed(format!(
                "Table collation: {}, Index collation: {}",
                name(own),
                name(&collation)
            )));
        }
    }
    let parent = table.owner.is_some().then_some(connection);
    let handle = register(session, table, parent)?;
    if let Some(opened) = opened {
        crate::trigger::attach(session, connection, handle, opened);
    }
    Ok(handle)
}

/// Opens a table as [`open`] does, for a client (`AdsOpenTable`): its
/// connection may keep the table's file open after the handle closes, if
/// it keeps tables open (`AdsCacheOpenTables`, see `crate::cache`).
pub fn open_for_client(
    session: &Session,
    connection: Handle,
    name: &str,
    mut settings: TableSettings,
) -> Result<Handle, Error> {
    // A running external procedure's `__input` and `__error`.
    let procedure_table = match connection {
        0 => None,
        _ => crate::aep::table_name(session, connection, name, &mut settings)?,
    };
    // On a dictionary's connection `ADS_TEMP_TABLE` opens the temporary
    // table of that name, as given (a `#` is part of a name it does not
    // find), where a free connection refuses it (`fixtures/sql/aeptables.json`;
    // only `__input` and `__error` were probed).
    let by_flag;
    let name = match procedure_table.as_deref() {
        None if connection != 0 && settings.options & options::TEMP_TABLE != 0 => {
            let owner = connection::get(session, connection)?;
            if owner.has_database() {
                if crate::temp::is_temp(name) || owner.temps.find(name).is_none() {
                    return Err(Error::FullText(7112).detailed(name));
                }
                settings.options &= !options::TEMP_TABLE;
                by_flag = format!("#{name}");
                &by_flag
            } else {
                name
            }
        }
        table => table.unwrap_or(name),
    };
    let resolved = settings.clone();
    // A client opens a temporary table as the type it was made as, and no
    // other (statements, which `open` serves too, are not so strict:
    // `fixtures/sql/temptables_api.json`).
    if connection != 0
        && crate::temp::is_temp(name)
        && let Ok(owner) = connection::get(session, connection)
    {
        let Some((_, made)) = owner.temps.find(name) else {
            return Err(Error::FullText(7112).detailed(crate::temp::bare(name)));
        };
        if settings.table_type == TableType::Database && !owner.has_database() {
            return Err(Error::InvalidTableType.detailed(
                "The default table type is not valid unless a data dictionary connection is \
                 used.",
            ));
        }
        if settings.table_type != TableType::Database {
            temp_type_mismatch(name, made, settings.table_type, owner.temps.pinned(name))?;
        }
    }
    let handle = open_as(session, connection, name, settings, true)
        .map_err(|error| with_file(session, connection, name, resolved, error))?;
    if procedure_table.is_some() {
        crate::aep::opened(connection, handle);
    } else if let Ok(table) = get(session, handle) {
        table.keep_open_as(crate::cache::Kind::Tables);
    }
    Ok(handle)
}

/// `error`, the failure to open the table `name`, with its file named as
/// the original engine names it in the text of a file it did not find or
/// could not open.
pub(crate) fn with_file(
    session: &Session,
    connection: Handle,
    name: &str,
    mut settings: TableSettings,
    error: Error,
) -> Error {
    match error {
        Error::FileNotFound | Error::FileInUse => {
            match resolve(session, connection, name, &mut settings) {
                Ok((path, ..)) => error.detailed(path.display().to_string()),
                Err(_) => error,
            }
        }
        error => error,
    }
}

/// The file of the table `name` on `connection`, with the connection
/// when there is one and the dictionary's entry for the table when it has
/// one; `settings` take the type a dictionary keeps for its table.
#[allow(clippy::type_complexity)]
fn resolve(
    session: &Session,
    connection: Handle,
    name: &str,
    settings: &mut TableSettings,
) -> Result<
    (
        PathBuf,
        Option<Arc<Connection>>,
        Option<crate::dictionary::Bound>,
    ),
    Error,
> {
    let (path, owner, bound) = match connection {
        // A server's clients reach tables through their connections only,
        // but for a path a map turns into one in a share.
        0 if let Some(shares) = &session.shares => match shares.mapped(name) {
            Some(path) => (path?, None, None),
            None => return Err(Error::FileNotOnServer),
        },
        0 => (PathBuf::from(name), None, None),
        _ => {
            let owner = connection::get(session, connection)?;
            // A temporary table is a file of the connection's own, of the
            // type it was made as.
            if let Some((path, table_type)) = owner.temps.find(name) {
                // A statement of another type than it was made as is refused
                // (`fixtures/sql/temptables_types.json`).
                let mut table_type = table_type;
                if settings.table_type != TableType::Database {
                    let pinned = owner.temps.pinned(name);
                    if !crate::temp::read_before(name)
                        && let Some(error) = crate::temp::type_mismatch(
                            name,
                            table_type,
                            settings.table_type,
                            pinned,
                            true,
                        )
                    {
                        return Err(Error::Sql(error));
                    }
                    crate::temp::note_read(name);
                    crate::temp::pin(&owner.temps, name, settings.table_type);
                    table_type = owner
                        .temps
                        .open_type(name, settings.table_type)
                        .unwrap_or(table_type);
                }
                settings.table_type = table_type;
                return Ok((path, Some(owner), None));
            }
            match owner
                .with_database_opt(|database| database.table(name))
                .flatten()
            {
                Some(bound) => {
                    // A dictionary's table is of the type the dictionary
                    // keeps, whatever the type asked for.
                    settings.table_type = bound.table_type;
                    settings.char_type = bound.char_type;
                    (bound.path.clone(), Some(owner), Some(bound))
                }
                None => {
                    let folder = owner.folder();
                    let path = match &session.shares {
                        Some(shares) => shares.inside(&folder, name)?,
                        None => crate::settings::local_join(session, &folder, name),
                    };
                    (path, Some(owner), None)
                }
            }
        }
    };
    let in_dictionary = owner.as_ref().is_some_and(|o| o.has_database());
    // Before the file is looked for: a name that is none is refused so too
    // (`fixtures/write/database_type`).
    if !in_dictionary && settings.table_type == TableType::Database {
        return Err(Error::InvalidTableType.detailed(
            "The default table type is not valid unless a data dictionary connection is used.",
        ));
    }
    if in_dictionary && bound.is_none() && settings.table_type == TableType::Database {
        settings.table_type = TableType::Adt;
    }
    let bare = path.extension().is_none() && !path.exists();
    let path = match settings.table_type {
        TableType::Adt if bare => path.with_extension("adt"),
        TableType::Ntx | TableType::Cdx | TableType::Vfp if bare => path.with_extension("dbf"),
        _ => path,
    };
    let path = if session.shares.is_some() {
        shares::in_any_case(path)
    } else {
        path
    };
    // Before the file is looked for too: a connection that checks free table
    // access refuses them to a user who is no administrator, whether or not
    // the file is there (`fixtures/dictionary/freetables.json`).
    if in_dictionary
        && bound.is_none()
        && owner
            .as_ref()
            .is_some_and(|owner| owner.refuses_free_table_open())
    {
        return Err(Error::Ace {
            code: 7087,
            detail: path.display().to_string(),
        });
    }
    // A name a dictionary has no table for, nor a file.
    if in_dictionary && bound.is_none() && !path.exists() {
        return Err(Error::InvalidObjectName);
    }
    Ok((path, owner, bound))
}

/// What a database trigger's `__info` tells of the table `name`, if it can be
/// told: its file, whether it is a free table, and its type.
pub(crate) fn open_info(
    session: &Session,
    connection: Handle,
    name: &str,
    settings: &TableSettings,
) -> Option<(PathBuf, bool, TableType)> {
    let mut settings = settings.clone();
    let (path, _, bound) = resolve(session, connection, name, &mut settings).ok()?;
    Some((path, bound.is_none(), settings.table_type))
}

/// The table `name` on `connection`, read but not yet behind a handle (see
/// [`open`]).
pub(crate) fn load(
    session: &Session,
    connection: Handle,
    name: &str,
    settings: TableSettings,
) -> Result<Table, Error> {
    let mut settings = settings;
    let (path, owner, bound) = resolve(session, connection, name, &mut settings)?;
    // An ADT table whose header names a dictionary is a database table, and
    // opens as one only: a free open of it is refused, its dictionary there
    // or not (`fixtures/write/encryption/freeopen.json`, `lone_plain`).
    if bound.is_none()
        && !OWN_FREE_OPEN.get()
        && matches!(settings.table_type, TableType::Adt | TableType::Database)
        && names_dictionary(&path)
    {
        return Err(Error::DatabaseTable.detailed(DATABASE_TABLE));
    }
    // An operation of maintenance on the table may close it to this session.
    crate::maintenance::admit(session, connection, &path)?;
    // A table a backup is copying is closed to the other connections.
    crate::backup::active::admit(session, &path)?;
    // A dictionary's table that lacks files is made again if its
    // auto-creation is on, else refused as an index file is
    // (`fixtures/dictionary/freetables.json`).
    if let Some(bound) = &bound
        && !REBUILDING.get()
        && (!crate::shares::actual_case(path.clone()).exists()
            || bound
                .index_files
                .iter()
                .any(|file| !crate::shares::actual_case(file.clone()).exists()))
    {
        crate::dictionary_write::auto_create(session, connection, bound.object_id)?;
        if !path.exists() {
            return Err(Error::Constraint {
                code: MISSING_INDEX_FILE,
                text: crate::ace_error::error_text(MISSING_INDEX_FILE)
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
    }
    // An ADT table of a free table's header, which carries no object number
    // of the dictionary, is not the dictionary's (a backup's image opened as
    // the table, say).
    if bound.is_some()
        && matches!(settings.table_type, TableType::Adt | TableType::Database)
        && stored_object_id(&path) == Some(0)
    {
        return Err(Error::Constraint {
            code: ID_MISMATCH,
            text: crate::ace_error::error_text(ID_MISMATCH)
                .unwrap_or_default()
                .to_owned(),
        });
    }
    // `resolve` leaves no table of the default type: a dictionary's table
    // has the type the dictionary keeps, and any other is refused.
    let format = match settings.table_type {
        TableType::Adt | TableType::Database => Format::Adt,
        TableType::Ntx => Format::Dbf(dbf::Flavour::Clipper),
        TableType::Cdx => Format::Dbf(dbf::Flavour::FoxPro),
        TableType::Vfp => Format::Dbf(dbf::Flavour::Visual),
    };
    // Transactions a crash left are finished or taken back first; one that
    // cannot be yet waits for the next table opened.
    let _ = crate::txlog::recover(session);
    // A dictionary-bound table is encrypted with the dictionary's own table
    // password, which the login password releases, or in a dictionary of AES
    // tables with its dictionary password (`DDPassword`); a free table opens
    // with the password the connection was given.
    let bound_owner = bound.as_ref().and(owner.as_ref());
    let aes_dictionary = bound_owner.is_some_and(|owner| {
        owner
            .with_database_opt(|database| database.scheme() != Scheme::Rc4)
            .unwrap_or(false)
    });
    let table_password = bound_owner.and_then(|owner| {
        owner
            .with_database_opt(|database| match database.scheme() {
                // The server has the login's password from the connect, and
                // a remote client sends none with the open.
                Scheme::Rc4 => {
                    database
                        .dictionary()
                        .table_password(match settings.password.is_empty() {
                            true => database.password(),
                            false => settings.password.as_bytes(),
                        })
                }
                _ => database.dd_password().map(<[u8]>::to_vec),
            })
            .flatten()
    });
    let password = table_password
        .as_deref()
        .or_else(|| (!settings.password.is_empty()).then_some(settings.password.as_bytes()));
    // A table of a dictionary that encrypts its structure (RC4), opened as a
    // free table without the table's password: the original cannot read the
    // structure, and says the password is wrong (7060, naming the file),
    // whatever password the connection has; while its dictionary has it open,
    // it reads the structure and refuses a dictionary's table (5159) (probed,
    // `fixtures/write/encryption/freeopen.json`).
    let free_rc4_structure =
        || format == Format::Adt && bound.is_none() && rc4_encrypted_structure(&path);
    // A table of an AES dictionary does not open free whatever the
    // connection's keys: its structure is not readable without the
    // dictionary (7068), and while the dictionary has it open it is a
    // dictionary's table (5159) (`fixtures/write/encryption/freeopen.json`,
    // `aes_t_*`, `aesc_t_*`).
    if format == Format::Adt
        && bound.is_none()
        && !OWN_FREE_OPEN.get()
        && aes_dictionary_table(&path)
    {
        return Err(match shared::in_use(&path) {
            true => Error::DatabaseTable.detailed(DATABASE_TABLE),
            false => Error::DictionaryEncrypted.detailed(path.display().to_string()),
        });
    }
    let file = shared::open(&path, format, password).map_err(|error| match error {
        StorageError::EncryptedDescriptors | StorageError::Encrypted if free_rc4_structure() => {
            Error::InvalidPassword.detailed(path.display().to_string())
        }
        // An encrypted table of an AES dictionary connected without its
        // dictionary password (`fixtures/aes/dict/aesdict3.json`).
        StorageError::EncryptedDescriptors if aes_dictionary && password.is_none() => {
            Error::DictionaryEncrypted
        }
        // The password a statement was given for a free table is not its.
        StorageError::Encrypted
            if settings.by_statement && bound.is_none() && password.is_some() =>
        {
            Error::InvalidPassword
        }
        // With the table's path, as the original engine reports it
        // (`fixtures/dbf/header_dates.json`).
        StorageError::Corrupt(tflip_storage::dbf::BAD_DATE) => {
            Error::FullText(7016).detailed(path.display().to_string())
        }
        error => structure_error(error),
    })?;
    let database_table = || Error::DatabaseTable.detailed(DATABASE_TABLE);
    // The password is the handle's, not the file's, which other handles may
    // have open already, with their password or without: this one's is
    // checked (and deciphers the file if none did yet), and without one a
    // table whose structure is encrypted does not open, as when it is not
    // open yet.
    let keyed = {
        let mut file = file.file();
        match password {
            Some(password) => match file.unsealed(|file| file.supply_password(password)) {
                Err(_) if free_rc4_structure() => return Err(database_table()),
                Err(StorageError::Encrypted) if settings.by_statement && bound.is_none() => {
                    return Err(Error::InvalidPassword);
                }
                keyed => keyed.map_err(structure_error)?,
            },
            None if file.has_password() && file.structure.header.encrypted_descriptors() => {
                return Err(match free_rc4_structure() {
                    true => database_table(),
                    false => Error::TableEncrypted,
                });
            }
            None => false,
        }
    };
    // A statement opens a free table that has a password only with it
    // (`fixtures/write/encryption/stmtpw.json`).
    if settings.by_statement && bound.is_none() && !keyed && file.file().has_password() {
        return Err(Error::TableEncrypted);
    }
    let exclusive = settings.options & options::EXCLUSIVE != 0;
    let mut table = build(session, connection, owner, path, settings, file)?;
    table.keyed = AtomicBool::new(keyed);
    lock(&table.state).file.attach(exclusive)?;
    table.attached = true;
    if let Some(bound) = bound {
        // A table whose rights slot for DB:Public does not check cannot be
        // opened, by anyone and before its rights are looked at
        // (`fixtures/sql/ddpublicbad`).
        if let Some(owner) = &table.owner
            && owner
                .with_database_opt(|database| {
                    (database.dictionary().object(bound.object_id))
                        .is_some_and(|object| database.public_slot_bad(object))
                })
                .unwrap_or(false)
        {
            return Err(Error::AgentFailed);
        }
        // As the original engine reports a dictionary's table opened by name
        // (`fixtures/sql/dbfdict.json`).
        table.settings.options |= options::DICTIONARY_BOUND_TABLE;
        // An index file the dictionary lists must be there
        // (`fixtures/sql/ddalterdbf`, after a restructure that left none).
        if !REBUILDING.get()
            && bound
                .index_files
                .iter()
                .any(|file| !crate::shares::actual_case(file.clone()).exists())
        {
            return Err(Error::Constraint {
                code: MISSING_INDEX_FILE,
                text: crate::ace_error::error_text(MISSING_INDEX_FILE)
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
        table.index_files = Some(bound.index_files);
        table.primary_key = bound.primary_key;
        table.object_id = Some(bound.object_id);
        let _ = table.alias.set(name.to_owned());
        // The referential integrity rules that bear on the table, enforced on
        // its writes (see `crate::ri`).
        table.ri = crate::ri::load(session, connection, name);
        // The dictionary's defaults, field constraints and record validation,
        // applied on its writes (see `crate::constraints`).
        table.checks = crate::constraints::load(
            session,
            connection,
            name,
            table.fields(),
            |expression| table.compile(expression),
            table.env(),
        );
        lock(&table.state).checks = table.checks.clone();
        // The rights the connection's user holds on the table. Opening at all
        // needs the read right; the original engine refuses otherwise (7079,
        // probed). Writing is gated per operation (see `Table::check_access`).
        if let Some(owner) = &table.owner
            && !owner.privileged()
        {
            table.access = owner
                .with_database_opt(|database| database.access_mask(name))
                .unwrap_or(u32::MAX);
            let changing = CHANGING.with_borrow(|changing| {
                changing.as_ref().is_some_and(|(table_name, right)| {
                    table_name.eq_ignore_ascii_case(name) && table.access & right != 0
                })
            });
            if table.access & crate::system::right::READ == 0 && !changing {
                return Err(Error::NoReadRights);
            }
            // The options report what the user may do with the table: not
            // what it may not, read-only if that is all it may do, and not
            // the bit every table without a hidden column has
            // (`fixtures/sql/ddcolrights`).
            use crate::system::right;
            for (right, permission) in [
                (right::UPDATE, options::PERM_UPDATE),
                (right::INSERT, options::PERM_INSERT),
                (right::DELETE, options::PERM_DELETE),
            ] {
                if table.access & right == 0 {
                    table.settings.options &= !permission;
                }
            }
            let permissions = options::PERM_READ
                | options::PERM_UPDATE
                | options::PERM_INSERT
                | options::PERM_DELETE;
            if table.settings.options & permissions == options::PERM_READ {
                table.settings.options |= options::READONLY;
            }
            table.sql_only = owner
                .with_database_opt(|database| database.permission_level_of(name))
                .flatten()
                == Some(3);
            if let Some(access) = owner
                .with_database_opt(|database| database.column_access(name))
                .flatten()
            {
                table.hidden = access.hidden();
                table.not_updatable = access.not_updatable();
                table.hidden_in_sql = access.level >= 2;
                if access.level == 2 && !access.lenient {
                    table.settings.options &= !options::OPENED;
                }
            }
        }
    }
    Ok(table)
}

/// A table from its parts.
pub(super) fn build(
    session: &Session,
    connection: Handle,
    owner: Option<Arc<Connection>>,
    path: PathBuf,
    settings: TableSettings,
    file: Arc<Shared>,
) -> Result<Table, Error> {
    let structure = file.file().structure.clone();
    let info = file.file().dbf().cloned();
    let dbf = matches!(structure.header.format, Format::Dbf(_));
    // A dictionary's table reopens as its format's type.
    let table_type = match (settings.table_type, structure.header.format) {
        (TableType::Database, Format::Dbf(tflip_storage::dbf::Flavour::Visual)) => TableType::Vfp,
        (TableType::Database, Format::Dbf(_)) => TableType::Cdx,
        (TableType::Database, _) => TableType::Adt,
        (table_type, _) => table_type,
    };
    let source = Arc::new(crate::txlog::Source {
        path: path.clone(),
        table_type,
        char_type: settings.char_type,
        collation: settings.collation.clone(),
    });
    let position = Position {
        record: None,
        bof: true,
        eof: true,
    };
    let state = Mutex::new(State {
        position,
        found: false,
        counted_past_end: false,
        file,
        pending: None,
        stranded: None,
        dropped: 0,
        kept: None,
        image: None,
        stale_count: None,
        owner: shared::next_owner(),
        orders: Vec::new(),
        deleted_shown: dbf.then(|| Arc::clone(&session.settings)),
        sequence: None,
        sorter: None,
        constrained: None,
        located: None,
        connection: owner.clone(),
        source,
        checks: None,
        hidden_slots: None,
    });
    // A table starts with its connection's "exact" setting.
    let exact = AtomicBool::new(crate::settings::exact_of(session, connection)?);
    let filters = Mutex::new(Arc::default());
    let session_settings = Arc::clone(&session.settings);
    let null_record = structure.null_record();
    Ok(Table {
        null_record,
        connection,
        path,
        settings,
        structure,
        state,
        owner,
        session_settings,
        exact,
        filters,
        statement: None,
        all_fields: None,
        cursor_order: None,
        built: false,
        uncounted: false,
        dbf: info,
        index_files: None,
        primary_key: None,
        object_id: None,
        alias: OnceLock::new(),
        opened: AtomicBool::new(false),
        stray_index: false,
        last_autoinc: AtomicU32::new(0),
        relations: Mutex::default(),
        following: Mutex::default(),
        attached: false,
        kept: std::sync::atomic::AtomicU8::new(0),
        access: u32::MAX,
        hidden: Vec::new(),
        not_updatable: Vec::new(),
        hidden_in_sql: false,
        sql_only: false,
        ri: crate::ri::Rules::default(),
        checks: None,
        keyed: AtomicBool::new(false),
        temporary_indexes: Mutex::default(),
    })
}

/// A table's name without folders and extension: the alias clients get by
/// default.
pub(crate) fn stem(name: &str) -> String {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
    file.split('.').next().unwrap_or(file).to_owned()
}

/// Puts `table` behind a new handle owned by `parent`, on its first record,
/// with its structural index orders (ADT tables) open.
pub(crate) fn register(
    session: &Session,
    mut table: Table,
    parent: Option<Handle>,
) -> Result<Handle, Error> {
    // A table opens on its first record, as in the original engine.
    table.goto_top()?;
    // The structural index of an ADT table opens with it, and that of a
    // FoxPro or VFP table whose header says it has one.
    let structural = match table.settings.table_type {
        TableType::Adt => table.statement_is_live(),
        TableType::Cdx | TableType::Vfp => {
            table.dbf.as_ref().is_some_and(|i| i.table_flags & 1 != 0)
        }
        _ => false,
    };
    // Only an exclusive handle refuses that file; a shared or read-only one
    // opens it (`fixtures/errors/stray_index_modes.json`).
    if matches!(table.settings.table_type, TableType::Cdx | TableType::Vfp)
        && !structural
        && table.exclusive()
    {
        let path = index::structural_path(&table.path);
        table.stray_index = matches!(
            tflip_storage::order::IndexFile::open(&path),
            Ok(tflip_storage::order::IndexFile::Cdx(cdx)) if !cdx.compound
        );
    }
    // An index built under a different static collation makes the open fail
    // (AE_INDEX_COLLATION_MISMATCH), before the table handle is created.
    let table = Arc::new(table);
    if structural || table.index_files.is_some() {
        index::check_collations(&table)?;
    }
    let handle = session.insert(Object::Table(Arc::clone(&table)), parent);
    match &table.index_files {
        // A dictionary's table opens the index files the dictionary has
        // for it.
        Some(files) => {
            for file in files {
                index::open_bound(session, handle, &table, file);
            }
        }
        None if structural => {
            index::open_structural(session, handle, &table);
        }
        None => {}
    }
    Ok(handle)
}

/// A table built in memory, `name` in `connection`'s folder: a static SQL
/// cursor of `statement`.
pub(crate) fn built(
    session: &Session,
    connection: Handle,
    statement: Handle,
    name: &str,
    settings: TableSettings,
    file: TableFile,
) -> Result<Table, Error> {
    let owner = connection::get(session, connection)?;
    let path = owner.path.join(name);
    let file = Shared::private(file);
    let mut table = build(session, connection, Some(owner), path, settings, file)?;
    table.statement = Some(statement);
    table.built = true;
    Ok(table)
}

/// A table built in memory to read as a `SELECT`'s source, `name` in
/// `connection`'s folder (a `system.*` table). It is not a cursor: the
/// `SELECT` reads it and builds its own cursor of the result.
pub(crate) fn source(
    session: &Session,
    connection: Handle,
    name: &str,
    settings: TableSettings,
    file: TableFile,
) -> Result<Table, Error> {
    let owner = connection::get(session, connection)?;
    let path = owner.path.join(name);
    let file = Shared::private(file);
    let mut table = build(session, connection, Some(owner), path, settings, file)?;
    table.built = true;
    Ok(table)
}

/// A data dictionary opened as a transient ADT table, to build its own index
/// (`.ai`) over its objects. It belongs to no connection; its records are the
/// dictionary's objects.
pub(crate) fn dictionary(session: &Session, path: &std::path::Path) -> Result<Table, Error> {
    let file =
        tflip_storage::table::TableFile::open_dictionary(path).map_err(|_| Error::CorruptTable)?;
    // An encrypted dictionary opens deciphered, or not at all.
    let keyed = file.structure.header.encrypted();
    let file = Shared::private(file);
    let mut table = build(
        session,
        0,
        None,
        path.to_path_buf(),
        TableSettings::default(),
        file,
    )?;
    table.built = true;
    table.keyed = AtomicBool::new(keyed);
    Ok(table)
}

/// A table opened on its own, belonging to no connection, to read its
/// structure and compile its expressions (e.g. to describe its indexes for a
/// data dictionary).
pub(crate) fn transient(
    session: &Session,
    path: &std::path::Path,
    format: Format,
) -> Result<Table, Error> {
    let file =
        tflip_storage::table::TableFile::open(path, format).map_err(|_| Error::CorruptTable)?;
    transient_of(session, path, format, Shared::private(file))
}

/// [`transient`] for a table that is encrypted, deciphered with `password`
/// (the dictionary password of an AES dictionary's tables).
pub(crate) fn transient_deciphered(
    session: &Session,
    path: &std::path::Path,
    format: Format,
    password: &[u8],
) -> Result<Table, Error> {
    let file =
        tflip_storage::table::TableFile::open_deciphering(path, format, false, Some(password))
            .map_err(structure_error)?;
    let keyed = file.structure.header.encrypted();
    let mut table = transient_of(session, path, format, Shared::private(file))?;
    table.keyed = AtomicBool::new(keyed);
    Ok(table)
}

fn transient_of(
    session: &Session,
    path: &std::path::Path,
    format: Format,
    file: Arc<Shared>,
) -> Result<Table, Error> {
    let table_type = match format {
        Format::Dbf(dbf::Flavour::Clipper) => TableType::Ntx,
        Format::Dbf(dbf::Flavour::FoxPro) => TableType::Cdx,
        Format::Dbf(dbf::Flavour::Visual) => TableType::Vfp,
        _ => TableType::Adt,
    };
    let settings = TableSettings {
        table_type,
        ..TableSettings::default()
    };
    let mut table = build(session, 0, None, path.to_path_buf(), settings, file)?;
    table.built = true;
    Ok(table)
}

/// Registers a table built in memory (`file`) as a standalone table handle
/// belonging to no connection, named `name`: how `AdsFindServers` returns its
/// in-memory result. The handle is an ordinary (ADT) table that
/// `AdsGetNumFields`, `AdsGetField`, `AdsGotoTop`, `AdsSkip` and
/// `AdsCloseTable` read like any other.
pub fn standalone(session: &Session, name: &str, file: TableFile) -> Result<Handle, Error> {
    let file = Shared::private(file);
    let mut table = build(
        session,
        0,
        None,
        PathBuf::from(name),
        TableSettings::default(),
        file,
    )?;
    table.built = true;
    register(session, table, None)
}

/// The table behind `handle`, which may also be one of the table's index
/// orders: ACE's record functions take a table or an index order handle.
pub fn get(session: &Session, handle: Handle) -> Result<Arc<Table>, Error> {
    session
        .with(handle, |object| match object {
            Object::Table(table) => Some(Arc::clone(table)),
            Object::Index(index) => Some(Arc::clone(index.owner())),
            _ => None,
        })
        .flatten()
        .ok_or(Error::InvalidTable)
}

/// Writes the pending changes of every table `session` has open
/// (`AdsWriteAllRecords`); the first failure is the result.
pub fn write_all(session: &Session) -> Result<(), Error> {
    let mut tables = Vec::new();
    session.for_each(|object| {
        if let Object::Table(table) = object {
            tables.push(Arc::clone(table));
        }
    });
    let mut result = Ok(());
    for table in tables {
        let written = table.write_record();
        if result.is_ok() {
            result = written;
        }
    }
    result
}

/// Closes the table behind `handle`.
pub fn close(session: &Session, handle: Handle) -> Result<(), Error> {
    let table = session.with(handle, |object| match object {
        Object::Table(table) => Some(Arc::clone(table)),
        _ => None,
    });
    let Some(Some(table)) = table else {
        return Err(Error::InvalidTable);
    };
    // Removing the table writes what is pending (see `Table::finish`).
    session.remove_if(handle, |object| matches!(object, Object::Table(_)));
    let closed = crate::trigger::table_closed(session, table.connection, handle);
    if let Some(statement) = table.statement {
        crate::statement::cursor_closed(session, statement, handle);
    }
    closed
}

/// Opens the table `handle` again (`AdsCloneTable`), with its settings, on
/// the record it is on.
pub fn clone_table(session: &Session, handle: Handle) -> Result<Handle, Error> {
    let table = get(session, handle)?;
    if table.statement.is_some() || table.built {
        return Err(Error::InvalidTable);
    }
    // By its name in the connection's folder: a server's sessions reach
    // files in their shares only.
    let full = table.path.to_string_lossy().into_owned();
    let name = match table.connection {
        0 => full,
        connection => {
            let folder = connection::get(session, connection)?.folder();
            table
                .path
                .strip_prefix(&folder)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or(full)
        }
    };
    let record = table.state().position.record;
    let clone = open(session, table.connection, &name, table.settings.clone())?;
    // The clone has the encryption password the table has.
    if table.keyed() {
        get(session, clone)?.keyed.store(true, Ordering::Relaxed);
    }
    if let Some(record) = record {
        get(session, clone)?.goto_record(record)?;
    }
    Ok(clone)
}

/// Closes the table `handle` and deletes its file, its memo file and its
/// structural index file (`AdsDeleteTable`).
pub fn delete(session: &Session, handle: Handle) -> Result<(), Error> {
    let table = get(session, handle)?;
    if table.statement.is_some() || table.built {
        return Err(Error::InvalidTable);
    }
    let extensions: &[&str] = match table.format() {
        Format::Adt => &["adm", "adi"],
        _ => &["dbt", "fpt", "cdx"],
    };
    let mut paths = vec![table.path.clone()];
    paths.extend(extensions.iter().map(|ext| table.path.with_extension(ext)));
    drop(table);
    close(session, handle)?;
    // Connections that keep the table open for reuse let go of it.
    crate::cache::evict_path(&paths[0]);
    for path in paths.iter().filter(|path| path.exists()) {
        std::fs::remove_file(path).map_err(|_| Error::FileInUse)?;
    }
    Ok(())
}
