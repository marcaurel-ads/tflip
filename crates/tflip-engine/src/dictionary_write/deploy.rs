//! Deploying a data dictionary to a folder of free tables
//! (`AdsDDDeployDatabase`), as the original engine does it
//! (`fixtures/dictionary/deploy`, made by `tools/ace/dd/deploy.c`).

use std::io::Write as _;

use tflip_storage::dd::{self, Dictionary, Kind, Object};
use tflip_storage::field::{Field, FieldType};

use super::tables::{dbf_field_options, recorded_length};
use super::*;

mod rebuild;
use crate::restructure::{self, Changes};
use crate::table::TableSettings;

/// The files a backup copies: the tables and their memo and index files. The
/// original leaves a dictionary's own files and anything else in the folder
/// where they are.
const BACKED_UP: [&str; 9] = [
    "adt", "adm", "adi", "dbf", "dbt", "fpt", "cdx", "ntx", "idx",
];

/// The deployment's log, `adsdeploy.log` in the destination: a line of the
/// time and the text per step, ended with CR LF.
struct Log(Option<std::fs::File>);

impl Log {
    fn line(&mut self, text: &str) {
        let now = chrono::Local::now().format("%H:%M:%S");
        self.raw(&format!("{now}  :  {text}"));
    }

    /// A line with no time, as the list of a restructure's changes has.
    fn raw(&mut self, text: &str) {
        if let Some(file) = &mut self.0 {
            let _ = write!(file, "{text}\r\n");
        }
    }
}

/// A path as the log shows it, in Windows' form.
fn shown(path: &str) -> String {
    path.replace('/', "\\")
}

/// A field as a dictionary keeps it.
struct Defined {
    name: String,
    field_type: FieldType,
    length: u16,
    decimals: u16,
    options: u32,
}

impl Defined {
    /// The fields of the table object `table`, in their order.
    fn of(dict: &Dictionary, table: &Object) -> Vec<Defined> {
        dict.children(table.id, Kind::Field)
            .into_iter()
            .filter_map(|field| {
                let definition = field.slot(dd::field::DEFINITION)?;
                let word = |at: usize| -> Option<u16> {
                    Some(u16::from_le_bytes([
                        *definition.get(at)?,
                        *definition.get(at + 1)?,
                    ]))
                };
                Some(Defined {
                    name: field.name.clone(),
                    field_type: FieldType::from_code(word(0)?),
                    length: word(2)?,
                    decimals: word(4)?,
                    options: field.u32(dd::field::OPTIONS).unwrap_or(0),
                })
            })
            .collect()
    }

    /// Whether the table's `field` is as this says.
    fn is(&self, field: &Field, adt: bool) -> bool {
        self.field_type == field.field_type
            && self.length == recorded_length(field)
            && self.decimals == field.decimals()
            && (adt || self.options == dbf_field_options(field))
    }

    /// The definition `AdsCreateTable` and `AdsRestructureTable` take: the
    /// length is the one the field was declared with, not the one a
    /// dictionary records for it.
    fn definition(&self, adt: bool) -> Result<String, Error> {
        let type_name = crate::ddl::type_name(self.field_type).ok_or(Error::NotImplemented)?;
        let length = match (adt, self.field_type) {
            (true, FieldType::VarChar | FieldType::VarBinary) => self.length.saturating_sub(2),
            (true, FieldType::NVarChar) => self.length.saturating_sub(1),
            _ => self.length,
        };
        let mut text = format!("{},{type_name},{length}", self.name);
        if self.decimals != 0 {
            text.push_str(&format!(",{}", self.decimals));
        }
        if self.options & 0b10 != 0 {
            text.push_str(",NULL");
        }
        if self.options & 0b01 != 0 {
            text.push_str(",NOCPTRANS");
        }
        Ok(text)
    }
}

/// How the deployment went on with a table.
enum Planned {
    /// There is a table to bind, with its object, its file and the files of
    /// its indexes.
    Table {
        object: u32,
        name: String,
        relative: String,
        path: PathBuf,
        table_type: TableType,
    },
    /// The deployment stops, with no error: a table of the dictionary that
    /// has no index file there has a production index in the folder.
    Stopped,
}

/// A free connection to the destination, closed when it goes.
struct Free<'a> {
    session: &'a Session,
    handle: Handle,
}

impl Drop for Free<'_> {
    fn drop(&mut self) {
        let _ = connection::disconnect(self.session, self.handle);
    }
}

/// The error 5019 for the parameter `n` of the call.
fn invalid_option(n: u32) -> Error {
    Error::InvalidOption.detailed(format!("Parameter {n}"))
}

/// The files that make a data dictionary: its `.add`, `.am` and `.ai`.
fn dictionary_files(add: &Path) -> [PathBuf; 3] {
    [
        add.to_path_buf(),
        dd::memo_path(add),
        add.with_extension("ai"),
    ]
}

/// Deploys the data dictionary at `source` to the folder `destination`, a
/// database of free tables (`AdsDDDeployDatabase`): the tables the dictionary
/// names are looked for there by their relative paths, a missing one is made
/// empty (and its folders), one with other fields is restructured to the
/// dictionary's, and the index orders it lacks are built; then the tables are
/// bound to a dictionary the folder gets, made anew from the source's objects
/// (see [`rebuild`]), and the source dictionary's files are renamed
/// `adsbackup`. `backup_files` copies the folder's tables first to its
/// `backup` folder, and `validate_option` 2 takes the records that break the
/// dictionary's constraints out of the tables into fail tables
/// (`move_failures`). The deployment's log is `adsdeploy.log` in the folder.
///
/// A table's rights granted to `DB:Public` stop the deployment with 5136 after
/// the groups are added ([`stop_on_public`]).
///
/// What is not done yet is a source dictionary with an administrator password
/// (the writers take the key from an empty one only), a field's message,
/// which the original never keeps, while it drops other properties of the
/// field by a rule not worked out, the limits of a DBF table's fields, which
/// the original drops in part, and the move of records that break the
/// constraints of a table with a memo field (`NOT_DONE` in the tests), all of
/// which are [`Error::NotImplemented`]. The destination's password is
/// ignored, as the original ignores it.
pub fn deploy_database_api(
    session: &Session,
    destination: &str,
    source: &str,
    source_password: &str,
    server_types: u16,
    validate_option: u16,
    backup_files: u16,
) -> Result<(), Error> {
    // The original looks at the options in the order of the parameters
    // from the last of the validations on, the server type second
    // (`fixtures/dictionary/deploy`).
    if !matches!(validate_option, 0 | 2) {
        return Err(invalid_option(6));
    }
    match server_types {
        1 | 2 => {}
        // The Internet server is one this process has no connection to.
        4 => return Err(Error::NoDriveConnection),
        _ => return Err(invalid_option(5)),
    }
    if backup_files > 1 {
        return Err(invalid_option(7));
    }
    let not_found = || ace_error::sql(5020, "Could not find dictionary file");
    let add = PathBuf::from(tflip_storage::universal_path(source));
    let folder = PathBuf::from(tflip_storage::universal_path(destination));
    if !add.is_file() || !folder.is_dir() {
        return Err(not_found());
    }
    let dict = Dictionary::open_with_password(&add, None).map_err(|error| match error {
        dd::Error::Encrypted => Error::DictionaryEncrypted,
        _ => Error::CorruptTable,
    })?;
    // The administrator's password opens the dictionary; the one given must
    // be it, or none if there is none.
    let given = source_password.as_bytes();
    let key = dict
        .dictionary_key(&given[..given.len().min(20)])
        .ok_or(Error::LoginFailed)?;
    let plan = rebuild::plan(&dict, &key)?;
    let tables = rebuild::tables(&dict);
    if tables
        .iter()
        .any(|table| cannot_deploy(&dict, table, validate_option))
    {
        return Err(Error::NotImplemented);
    }

    // A table of a dictionary cannot be a free one's, wherever it is.
    check_free(&folder)?;

    let shown_folder = shown(destination.trim_end_matches(['/', '\\']));
    let mut log = Log(std::fs::File::create(folder.join("adsdeploy.log")).ok());
    log.line("Starting Deployment...");
    let fail_folder = folder.join("fail");
    let _ = std::fs::create_dir(&fail_folder);
    log.line(&format!(
        "Creating fail table directory {shown_folder}\\fail."
    ));

    let file_name = add.file_name().ok_or_else(not_found)?;
    let deployed = folder.join(file_name);
    if deployed.exists() {
        let error = ace_error::sql(5128, "");
        log.line(
            "Error 5128:  The data dictionary cannot be created because the file already exists.",
        );
        return Err(error);
    }

    if backup_files == 1 {
        back_up(&folder, &shown_folder, &mut log);
    }
    let database = dict.database().ok_or(Error::CorruptTable)?;
    for text in settings_before(database) {
        log.line(&text);
    }

    let dictionary_file = file_name.to_string_lossy().into_owned();
    let free = Free {
        session,
        handle: connection::connect_path(
            session,
            &folder.to_string_lossy(),
            connection::LOCAL,
            None,
            None,
        )?,
    };
    let mut planned = Vec::new();
    for table in &tables {
        match analyse(
            &free,
            &dict,
            table,
            &folder,
            &shown_folder,
            &dictionary_file,
            &mut log,
        )? {
            Planned::Stopped => {
                // What the original leaves of a deployment it stops: the
                // destination has an empty dictionary of the source's name.
                drop(free);
                write_dictionary(session, &deployed, "", None, false)?;
                // With the logins and the rights it makes the others have.
                let mut file = open_dictionary_file(&deployed)?;
                edit_object_properties(&mut file, &deployed, 1, |slots| {
                    set_slot(slots, dd::database::LOGIN_NOT_REQUIRED, None);
                    set_slot(slots, dd::database::RIGHTS_NOT_CHECKED, None);
                    set_slot(slots, dd::database::QUERY_VIA_ROOT, Some(&[0; 4]));
                })?;
                return Ok(());
            }
            table => planned.push(table),
        }
    }

    log.line("Starting to add tables to dictionary.");
    for table in &planned {
        let Planned::Table {
            object,
            name,
            relative,
            path,
            table_type,
        } = table
        else {
            continue;
        };
        log.line(&format!("Adding table {name} to the dictionary."));
        let indexes = dict.children(*object, Kind::IndexFile);
        build_orders(
            &free,
            &dict,
            *object,
            &folder,
            relative,
            *table_type,
            &indexes,
            "",
        )?;
        for file in &indexes {
            let relative = file.text(dd::index_file::RELATIVE_PATH).unwrap_or_default();
            log.line(&format!(
                "Adding Index {shown_folder}\\{relative} to the dictionary."
            ));
        }
        if *table_type == TableType::Adt {
            let id = plan.id(*object).ok_or(Error::CorruptTable)?;
            dd::create::bind_table(path, id, &dictionary_file).map_err(|_| Error::CorruptTable)?;
            // A table in a folder of the destination names the dictionary with
            // one more dot for each folder it is down, not by `..\` as a path
            // (`fixtures/dictionary/deploy/subm_b`).
            let depth = strip_relative(relative).split(['\\', '/']).count() - 1;
            if depth > 0 {
                let reference = format!("{}\\{dictionary_file}", ".".repeat(depth + 1));
                dd::create::set_back_reference(path, &reference)
                    .map_err(|_| Error::CorruptTable)?;
            }
        }
    }
    drop(free);
    log_objects(&dict, &mut log, plan.fails_on_public);
    if plan.fails_on_public {
        return stop_on_public(
            session, &dict, key, &plan, &add, &deployed, &planned, &mut log,
        );
    }
    for text in settings_after(database) {
        log.line(&text);
    }

    // The folder gets the dictionary made anew, and the source's files are put
    // aside.
    rebuild::write(session, &dict, key, &plan, &add, &deployed, &[])?;
    make_queues(&dict, &plan, &deployed)?;
    if validate_option == 2 {
        move_failures(session, &deployed, &folder, &dict, &tables)?;
    }
    let source_files = dictionary_files(&add);
    let aside = dictionary_files(&add.with_file_name("adsbackup.add"));
    for (from, to) in source_files.iter().zip(&aside) {
        std::fs::rename(from, to).map_err(|_| Error::FileCreate)?;
    }
    Ok(())
}

/// The files of the queues the dictionary's subscriptions came with, made in
/// the folder of the `deployed` dictionary.
fn make_queues(dict: &Dictionary, plan: &rebuild::Plan, deployed: &Path) -> Result<(), Error> {
    let folder = deployed.parent().unwrap_or_else(|| Path::new("."));
    let file_name = deployed.file_name().ok_or(Error::CorruptTable)?;
    for subscription in dict.all(Kind::Subscription) {
        let Some(queue) = rebuild::queue_table(dict, subscription) else {
            continue;
        };
        let (Some(id), Some(table_path), Some(index_path)) = (
            plan.id(queue.id),
            queue.text(dd::table::RELATIVE_PATH),
            dict.children(queue.id, Kind::IndexFile)
                .first()
                .and_then(|file| file.text(dd::index_file::RELATIVE_PATH)),
        ) else {
            continue;
        };
        replication::make_deployed_queue(
            folder,
            &file_name.to_string_lossy(),
            &table_path,
            &index_path,
            id,
        )?;
    }
    Ok(())
}

/// The files the dictionary's table `object` lacks (its file, its index
/// files and, with the table, its memo file), made empty as they were when
/// the table's auto-creation is on (`Table_Auto_Create`;
/// `fixtures/dictionary/freetables.json`), as the original makes them when
/// the table is opened.
pub(crate) fn auto_create(session: &Session, connection: Handle, object: u32) -> Result<(), Error> {
    let conn = connection::get(session, connection)?;
    let Some(file_name) = conn
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return Ok(());
    };
    let Some((dict, folder)) =
        conn.with_database_opt(|database| (database.dictionary().clone(), database.path("")))
    else {
        return Ok(());
    };
    let Some(table) = dict.object(object) else {
        return Ok(());
    };
    if table.has(dd::table::NO_AUTO_CREATE) {
        return Ok(());
    }
    let relative = table
        .text(dd::table::RELATIVE_PATH)
        .ok_or(Error::CorruptTable)?;
    let path = under_folder(&folder, &relative);
    let indexes = dict.children(table.id, Kind::IndexFile);
    let table_missing = !path.exists();
    let index_missing = indexes.iter().any(|file| {
        file.text(dd::index_file::RELATIVE_PATH)
            .is_some_and(|relative| !under_folder(&folder, &relative).exists())
    });
    if !table_missing && !index_missing {
        return Ok(());
    }
    let free = Free {
        session,
        handle: connection::connect_path(
            session,
            &folder.to_string_lossy(),
            connection::LOCAL,
            None,
            None,
        )?,
    };
    let table_type = crate::dictionary::table_type(table);
    if table_missing {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| Error::FileCreate)?;
        }
        make_table(&free, table, table_type, &Defined::of(&dict, table))?;
        if table.info2.unwrap_or(0) & dd::table::flags::TRANSACTION_FREE != 0 {
            crate::ddl::set_file_transaction_free(&path, true)?;
        }
    }
    crate::table::opening_free_itself(|| {
        build_orders(
            &free, &dict, table.id, &folder, &relative, table_type, &indexes, "",
        )
    })?;
    if table_missing && table_type == TableType::Adt {
        dd::create::bind_table(&path, table.id, &file_name).map_err(|_| Error::CorruptTable)?;
        let depth = strip_relative(&relative).split(['\\', '/']).count() - 1;
        if depth > 0 {
            let reference = format!("{}\\{file_name}", ".".repeat(depth + 1));
            dd::create::set_back_reference(&path, &reference).map_err(|_| Error::CorruptTable)?;
        }
    }
    Ok(())
}

/// Whether the dictionary's `table` has a constraint an existing record may
/// break: a field's minimum, maximum or NOT NULL, or the table's own
/// validation expression.
fn has_constraints(dict: &Dictionary, table: &Object) -> bool {
    table.has(dd::table::VALIDATION_EXPR)
        || dict.children(table.id, Kind::Field).iter().any(|field| {
            [
                dd::field::MIN_VALUE,
                dd::field::MAX_VALUE,
                dd::field::NOT_NULL,
            ]
            .iter()
            .any(|slot| field.has(*slot))
        })
}

/// Whether the deployment looks for records that break a table's constraints:
/// in an ADT or a Visual FoxPro table; those of a Clipper or FoxPro one are
/// left as they are (`fixtures/dictionary/deploy/d_*`).
fn validated(dict: &Dictionary, table: &Object) -> bool {
    matches!(
        crate::dictionary::table_type(table),
        TableType::Adt | TableType::Vfp
    ) && has_constraints(dict, table)
}

/// Whether the deployment cannot do for `table` what the original does: the
/// dictionary it makes has not the limits of a Clipper's or FoxPro's fields,
/// nor those of a character field of a Visual FoxPro table, by a rule not
/// worked out; and the records of an ADT or Visual FoxPro table with a memo
/// field that it takes out as `validate_option` 2 asks are not moved to a
/// fail table yet (`fixtures/dictionary/deploy`, the `d_` scenarios).
fn cannot_deploy(dict: &Dictionary, table: &Object, validate_option: u16) -> bool {
    (crate::dictionary::table_type(table) != TableType::Adt && has_constraints(dict, table))
        || (validate_option == 2
            && validated(dict, table)
            && Defined::of(dict, table)
                .iter()
                .any(|field| field.field_type.is_memo()))
}

/// Takes the records of the deployed tables that break the dictionary's
/// constraints out of them, as the original does for `validate_option` 2: each
/// is deleted, and a copy is put in the table's fail table, `<table>_fail`
/// in the `fail` folder, which a deployment makes anew
/// (`fixtures/dictionary/deploy/v_*`).
fn move_failures(
    session: &Session,
    deployed: &Path,
    folder: &Path,
    dict: &Dictionary,
    tables: &[&Object],
) -> Result<(), Error> {
    let owner = Free {
        session,
        handle: connection::connect_path(
            session,
            &deployed.to_string_lossy(),
            connection::LOCAL,
            Some("ADSSYS"),
            Some(""),
        )?,
    };
    let free = Free {
        session,
        handle: connection::connect_path(
            session,
            &folder.to_string_lossy(),
            connection::LOCAL,
            None,
            None,
        )?,
    };
    for object in tables.iter().filter(|t| validated(dict, t)) {
        let handle = table::open(
            session,
            owner.handle,
            &object.name,
            TableSettings {
                options: table::options::EXCLUSIVE,
                ..TableSettings::default()
            },
        )?;
        // The original takes the records out one constraint at a time and
        // makes the fail table anew for each, so only those the last one found
        // stay in it.
        let failed = {
            let table = table::get(session, handle)?;
            let mut passes: std::collections::BTreeMap<crate::constraints::Broken, Vec<u32>> =
                std::collections::BTreeMap::new();
            for slot in 1..=table.record_count()? {
                table.goto_record(slot)?;
                if !table.is_deleted()?
                    && let Some(broken) = table.broken_constraint()?
                {
                    passes.entry(broken).or_default().push(slot);
                }
            }
            let mut last = Vec::new();
            for slots in passes.into_values() {
                last.clear();
                for slot in slots {
                    table.goto_record(slot)?;
                    last.push(table.read_record()?);
                    table.delete_record()?;
                }
            }
            last
        };
        table::close(session, handle)?;
        if failed.is_empty() {
            continue;
        }
        let name = format!("fail/{}_fail", object.name);
        let _ = std::fs::remove_file(under_folder(folder, &format!("{name}.adt")));
        let fields = Defined::of(dict, object)
            .iter()
            .map(|field| field.definition(true))
            .collect::<Result<Vec<_>, _>>()?
            .join(";");
        let block_size = object.u16(dd::table::MEMO_BLOCK_SIZE).unwrap_or(0);
        let created = crate::ddl::create_table_api(
            session,
            free.handle,
            &name,
            &fields,
            block_size,
            TableSettings::default(),
        )?;
        {
            let table = table::get(session, created)?;
            for record in &failed {
                table.append()?;
                table.set_record(record)?;
                table.write_record()?;
            }
        }
        table::close(session, created)?;
    }
    Ok(())
}

/// 5159 if a table in the destination (or one of its tables' folders) is one
/// of a dictionary: the original opens them as free tables.
fn check_free(folder: &Path) -> Result<(), Error> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let adt = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("adt"));
        if adt && dd::create::is_bound(&path).unwrap_or(false) {
            return Err(Error::Sql(tflip_sql::SqlError::ace(
                5159,
                "Error encountered when trying to open a database table.",
                table::DATABASE_TABLE,
            )));
        }
    }
    Ok(())
}

/// Copies the destination's table files into its `backup` folder, leaving
/// those that are there already, as the original does, with a word in the
/// log.
fn back_up(folder: &Path, shown_folder: &str, log: &mut Log) {
    let backup = folder.join("backup");
    log.line(&format!(
        "Creating backup directory {shown_folder}\\backup\\."
    ));
    if std::fs::create_dir(&backup).is_err() {
        log.line(&format!(
            "Unable to create backup directory: {shown_folder}\\backup\\."
        ));
    }
    if let Ok(entries) = std::fs::read_dir(folder) {
        let mut files: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
        files.sort();
        for path in files {
            let extension = path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase());
            if !path.is_file() || !BACKED_UP.iter().any(|e| Some(*e) == extension.as_deref()) {
                continue;
            }
            let Some(name) = path.file_name() else {
                continue;
            };
            let to = backup.join(name);
            if to.exists() || std::fs::copy(&path, &to).is_err() {
                let name = name.to_string_lossy();
                log.line(&format!(
                    "Error copying {shown_folder}\\{name} to {shown_folder}\\backup\\{name}"
                ));
            }
        }
    }
    log.line("Done with backups");
}

/// The table type's format in the storage layer.
fn format(table_type: TableType) -> tflip_storage::table::Format {
    use tflip_storage::dbf::Flavour;
    use tflip_storage::table::Format;
    match table_type {
        TableType::Ntx => Format::Dbf(Flavour::Clipper),
        TableType::Cdx => Format::Dbf(Flavour::FoxPro),
        TableType::Vfp => Format::Dbf(Flavour::Visual),
        _ => Format::Adt,
    }
}

/// Looks at the dictionary's table `table` in the destination: makes the
/// file if it is not there, and changes the fields of one that is to the
/// dictionary's.
fn analyse(
    free: &Free,
    dict: &Dictionary,
    table: &Object,
    folder: &Path,
    shown_folder: &str,
    dictionary_file: &str,
    log: &mut Log,
) -> Result<Planned, Error> {
    let relative = table
        .text(dd::table::RELATIVE_PATH)
        .ok_or(Error::CorruptTable)?;
    let path = under_folder(folder, &relative);
    log.line(&format!("Analyzing {shown_folder}{relative}"));
    let table_type = crate::dictionary::table_type(table);
    let adt = table_type == TableType::Adt;
    let defined = Defined::of(dict, table);
    let indexes = dict.children(table.id, Kind::IndexFile);
    let planned = Planned::Table {
        object: table.id,
        name: table.name.clone(),
        relative: relative.clone(),
        path: path.clone(),
        table_type,
    };
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| Error::FileCreate)?;
        }
        make_table(free, table, table_type, &defined)?;
        return Ok(planned);
    }
    // A production index file that the dictionary does not know stops the
    // original, which says nothing of it.
    let production = path.with_extension(if adt { "adi" } else { "cdx" });
    if indexes.is_empty() && table_type != TableType::Ntx && production.exists() {
        return Ok(Planned::Stopped);
    }
    let existing = table::transient(free.session, &path, format(table_type))?;
    let fields = existing.fields().to_vec();
    drop(existing);
    let (mut add, mut delete, mut change) = (Vec::new(), Vec::new(), Vec::new());
    let (mut shown_add, mut shown_delete, mut shown_change) =
        (String::new(), String::new(), String::new());
    let lower = |text: &str, name: &str| {
        let (head, rest) = text.split_at(name.len());
        format!("{head}{}", rest.to_ascii_lowercase())
    };
    for field in &defined {
        let found = fields
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(&field.name));
        match found {
            None => {
                let text = field.definition(adt)?;
                shown_add.push_str(&format!("{};", lower(&text, &field.name)));
                add.push(text);
            }
            Some(old) if !field.is(old, adt) => {
                let text = field.definition(adt)?;
                shown_change.push_str(&format!("{},{};", old.name, lower(&text, &field.name)));
                change.push(format!("{},{text}", old.name));
            }
            Some(_) => {}
        }
    }
    for old in &fields {
        if !defined
            .iter()
            .any(|f| f.name.eq_ignore_ascii_case(&old.name))
        {
            shown_delete.push_str(&format!("{};", old.name));
            delete.push(old.name.clone());
        }
    }
    if add.is_empty() && delete.is_empty() && change.is_empty() {
        return Ok(planned);
    }
    log.line(&format!("Restructuring {shown_folder}{relative}"));
    log.raw(&format!("  Adding: {shown_add}"));
    log.raw(&format!("  Deleting: {shown_delete}"));
    log.raw(&format!("  Changing: {shown_change}"));
    let mut changes = Changes::parse(
        &add.join(";"),
        &delete.join(";"),
        &change.join(";"),
        0,
        table_type,
    )?;
    changes.bind = adt.then(|| (table.id, dictionary_file.to_owned()));
    changes.indexes = indexes
        .iter()
        .filter_map(|file| file.text(dd::index_file::RELATIVE_PATH))
        .map(|file| under_folder(folder, &file))
        .collect();
    changes.expressions = dict
        .children(table.id, Kind::Index)
        .into_iter()
        .filter_map(|key| {
            let file = dict.object(key.info1? as u32)?;
            Some((
                under_folder(folder, &file.text(dd::index_file::RELATIVE_PATH)?),
                key.text(dd::index::EXPRESSION)?,
                key.text(dd::index::CONDITION).unwrap_or_default(),
            ))
        })
        .collect();
    let settings = TableSettings {
        table_type,
        ..TableSettings::default()
    };
    let name = relative_name(&relative);
    restructure::restructure(free.session, free.handle, &name, settings, &changes)?;
    Ok(planned)
}

/// A table's file as its connection names it: the dictionary's relative
/// path, with `/` for the folder separator and no leading `.\`.
fn relative_name(relative: &str) -> String {
    strip_relative(relative).replace('\\', "/")
}

/// Makes the empty table of the dictionary's `table` with its `defined`
/// fields.
fn make_table(
    free: &Free,
    table: &Object,
    table_type: TableType,
    defined: &[Defined],
) -> Result<(), Error> {
    let adt = table_type == TableType::Adt;
    let fields = defined
        .iter()
        .map(|field| field.definition(adt))
        .collect::<Result<Vec<_>, _>>()?
        .join(";");
    let block_size = table.u16(dd::table::MEMO_BLOCK_SIZE).unwrap_or(0);
    let oem = table.info2.unwrap_or(0) & dd::table::flags::OEM != 0;
    let settings = TableSettings {
        table_type,
        char_type: if oem && !adt { 2 } else { 1 },
        ..TableSettings::default()
    };
    let name = relative_name(
        &table
            .text(dd::table::RELATIVE_PATH)
            .ok_or(Error::CorruptTable)?,
    );
    let handle = crate::ddl::create_table_api(
        free.session,
        free.handle,
        &name,
        &fields,
        block_size,
        settings,
    )?;
    table::close(free.session, handle)
}

/// Builds the index files of the dictionary's table `object`, which `folder`
/// holds as a table without them (a restore's), as a free connection of the
/// engine's own; `password` is the table's, if it is encrypted.
pub(crate) fn build_restored_orders(
    session: &Session,
    dict: &Dictionary,
    object: u32,
    folder: &Path,
    relative: &str,
    table_type: TableType,
    password: &str,
) -> Result<(), Error> {
    let free = Free {
        session,
        handle: connection::connect_path(
            session,
            &folder.to_string_lossy(),
            connection::LOCAL,
            None,
            None,
        )?,
    };
    let indexes = dict.children(object, Kind::IndexFile);
    crate::table::opening_free_itself(|| {
        build_orders(
            &free, dict, object, folder, relative, table_type, &indexes, password,
        )
    })
}

/// Brings the index files of the table `relative` in the destination `folder`
/// to the dictionary's `indexes`: the orders a file lacks are built, a file
/// that is not there is made, and an order of a file that the dictionary does
/// not have is dropped (`fixtures/dictionary/deploy/idx_other`).
#[allow(clippy::too_many_arguments)]
fn build_orders(
    free: &Free,
    dict: &Dictionary,
    object: u32,
    folder: &Path,
    relative: &str,
    table_type: TableType,
    indexes: &[&Object],
    password: &str,
) -> Result<(), Error> {
    use tflip_storage::order::IndexFile;
    let keys = dict.children(object, Kind::Index);
    let mut handle = None;
    for file in indexes {
        let file_relative = file
            .text(dd::index_file::RELATIVE_PATH)
            .ok_or(Error::CorruptTable)?;
        let index_path = under_folder(folder, &file_relative);
        let present: Vec<String> = match index_path.exists() {
            true => {
                let stem = index_path.file_stem().unwrap_or_default();
                IndexFile::open(&index_path)
                    .map(|index| index.tags(&stem.to_string_lossy()))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|tag| tag.name)
                    .collect()
            }
            false => Vec::new(),
        };
        let wanted: Vec<&&Object> = keys
            .iter()
            .filter(|key| key.info1 == Some(file.id as i32))
            .collect();
        let named =
            |names: &[String], name: &str| names.iter().any(|n| n.eq_ignore_ascii_case(name));
        let missing: Vec<&&&Object> = wanted
            .iter()
            .filter(|key| !named(&present, &key.name))
            .collect();
        // Only a file of several orders drops one: a Clipper file is the
        // one order.
        let compound = index_path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("adi") || e.eq_ignore_ascii_case("cdx"));
        let extra: Vec<&String> = present
            .iter()
            .filter(|tag| compound && !wanted.iter().any(|key| key.name.eq_ignore_ascii_case(tag)))
            .collect();
        if missing.is_empty() && extra.is_empty() {
            continue;
        }
        let table_handle = match handle {
            Some(handle) => handle,
            None => {
                let opened = table::open(
                    free.session,
                    free.handle,
                    &relative_name(relative),
                    TableSettings {
                        table_type,
                        options: table::options::EXCLUSIVE,
                        password: password.to_owned(),
                        ..TableSettings::default()
                    },
                )?;
                handle = Some(opened);
                opened
            }
        };
        let page_size = u32::from(file.u16(dd::index_file::PAGE_SIZE).unwrap_or(0));
        for key in missing {
            let created = index::create(
                free.session,
                table_handle,
                Some(&relative_name(&file_relative)),
                &key.name,
                &key.text(dd::index::EXPRESSION).unwrap_or_default(),
                &key.text(dd::index::CONDITION).unwrap_or_default(),
                key.u32(dd::index::OPTIONS).unwrap_or(0),
                page_size,
            )?;
            free.session.remove(created);
        }
        for name in extra {
            let order = index::orders_of(free.session, table_handle)?
                .into_iter()
                .find(|order| {
                    index::get(free.session, *order).is_ok_and(|index| {
                        index.path == index_path && index.tag.name.eq_ignore_ascii_case(name)
                    })
                });
            if let Some(order) = order {
                index::delete(free.session, order)?;
            }
        }
    }
    if let Some(handle) = handle {
        table::close(free.session, handle)?;
    }
    Ok(())
}

/// What the original does when it grants the rights of `DB:Public` after the
/// groups are added and finds a table's: it frees the tables again, but those
/// of a referential integrity rule ("Could not free table"), answers 5136 and
/// leaves the dictionary with what was added before the users, the tables'
/// objects and their rights gone (`fixtures/dictionary/deploy/x_pubt*`). The
/// source's files stay as they were.
#[allow(clippy::too_many_arguments)]
fn stop_on_public(
    session: &Session,
    dict: &Dictionary,
    key: Vec<u8>,
    plan: &rebuild::Plan,
    add: &Path,
    deployed: &Path,
    planned: &[Planned],
    log: &mut Log,
) -> Result<(), Error> {
    let in_rule = |table: &Object| {
        dict.all(Kind::Relation).iter().any(|rule| {
            [dd::relation::PRIMARY_TABLE, dd::relation::FOREIGN_TABLE]
                .iter()
                .any(|&slot| rule.u32(slot) == Some(table.id))
        })
    };
    let mut freed = Vec::new();
    for table in rebuild::tables(dict) {
        match in_rule(table) {
            true => log.line("Could not free table\r\n"),
            false => freed.push(table),
        }
    }
    // The queues of the subscriptions are freed with the tables, after them.
    let queues: Vec<&Object> = dict
        .all(Kind::Subscription)
        .iter()
        .filter_map(|subscription| rebuild::queue_table(dict, subscription))
        .collect();
    freed.extend(&queues);
    rebuild::write(session, dict, key, plan, add, deployed, &freed)?;
    make_queues(dict, plan, deployed)?;
    let folder = deployed.parent().unwrap_or_else(|| Path::new("."));
    for queue in queues {
        let own = queue
            .text(dd::table::RELATIVE_PATH)
            .and_then(|path| path.strip_prefix(".\\").map(str::to_owned))
            .filter(|file| !file.contains(['\\', '/']));
        if let Some(file) = own {
            let path = folder.join(file);
            if path.exists() {
                crate::cache::evict_path(&path);
                dd::create::free_table(&path).map_err(|_| Error::CorruptTable)?;
            }
        }
    }
    for planned in planned {
        if let Planned::Table {
            object,
            path,
            table_type: TableType::Adt,
            ..
        } = planned
            && freed.iter().any(|table| table.id == *object)
        {
            crate::cache::evict_path(path);
            dd::create::free_table(path).map_err(|_| Error::CorruptTable)?;
        }
    }
    let detail = "Column level permission is not supported by the DB:Public role.";
    log.line(&format!(
        "Error 5136:  The requested object type is not valid.  {detail}"
    ));
    Err(ace_error::sql(5136, detail))
}

/// The log's word for each object the dictionary holds that it tells of, in
/// the order they are added.
fn log_objects(dict: &Dictionary, log: &mut Log, groups_only: bool) {
    for (kind, word) in [
        (Kind::View, "view"),
        (Kind::Link, "link"),
        (Kind::Procedure, "stored procedure"),
        (Kind::Trigger, "trigger"),
        (Kind::Relation, "RI object"),
        (Kind::Publication, "publication"),
        (Kind::Subscription, "subscription"),
        (Kind::Group, "user group"),
        (Kind::User, "user"),
    ] {
        if groups_only && kind == Kind::User {
            break;
        }
        for object in dict.all(kind) {
            // The rules of integrity are told of as the tables are.
            let verb = if kind == Kind::Relation {
                "Adding"
            } else {
                "Added"
            };
            log.line(&format!("{verb} {word} {} to the dictionary.", object.name));
        }
    }
}

/// The log's lines about the settings of the source's database that the
/// deployment starts with.
fn settings_before(database: &Object) -> Vec<String> {
    let truth = |on: bool| if on { "TRUE" } else { "FALSE" };
    vec![
        "Turning verify rights on.".to_owned(),
        "Turning logins required on.".to_owned(),
        format!(
            "Setting table encryption to {}",
            truth(database.has(dd::database::ENCRYPT_NEW_TABLE))
        ),
        format!(
            "Setting index encryption to {}",
            truth(database.has(dd::database::ENCRYPT_INDEXES))
        ),
        // The log calls the encryption of the communication the index's too.
        format!(
            "Setting index encryption to {}",
            truth(database.has(dd::database::ENCRYPT_COMMUNICATION))
        ),
        format!(
            "Setting query via root option to {}",
            truth(database.u32(dd::database::QUERY_VIA_ROOT).unwrap_or(0) != 0)
        ),
    ]
}

/// The log's lines about the settings the deployed database has.
fn settings_after(database: &Object) -> Vec<String> {
    let flag = |slot: usize| database.u16(slot) == Some(1);
    let on = |on: bool| if on { "ON" } else { "OFF" };
    let level = database
        .u16(dd::database::INTERNET_SECURITY_LEVEL)
        .unwrap_or(2);
    let attempts = database.u16(dd::database::MAX_FAILED_ATTEMPTS).unwrap_or(5);
    vec![
        "Setting dictionary password back.".to_owned(),
        "Setting access rights to : OFF".to_owned(),
        format!(
            "Setting logins required to : {}",
            on(rebuild::logins_required(database))
        ),
        format!(
            "Turning {} internet access.",
            on(flag(dd::database::ENABLE_INTERNET))
        ),
        format!("Setting security level to {}.", level.saturating_sub(1)),
        format!("Turning {} adssys internet access.", on(flag(10))),
        format!("Setting max failed login attempts to {attempts}."),
        format!(
            "Turning {} enforce max failed logins.",
            on(flag(dd::database::ENFORCE_MAX_FAILED_LOGINS))
        ),
        match flag(dd::database::LOGINS_DISABLED) {
            true => "Disabling database logins.".to_owned(),
            false => "Enabling database logins.".to_owned(),
        },
        "Setting 'logins disabled' custom error string.".to_owned(),
        "Setting FTS defaults.".to_owned(),
        match database.has(dd::database::DISABLE_DLL_CACHING) {
            true => "Disabling DLL Caching.".to_owned(),
            false => "Enabling DLL Caching.".to_owned(),
        },
        match database.has(dd::database::TRIGGERS_DISABLED) {
            true => "Disabling Triggers.".to_owned(),
            false => "Enabling Triggers.".to_owned(),
        },
        "Closed connection.".to_owned(),
        "Deployment done.".to_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use super::test_support::{mask_db_secret, mask_login, mask_public_rights, public_rights_of};
    use super::*;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/dictionary/deploy")
    }

    fn copy_all(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let path = entry.unwrap().path();
            let target = to.join(path.file_name().unwrap());
            match path.is_dir() {
                true => copy_all(&path, &target),
                false => drop(std::fs::copy(&path, &target).unwrap()),
            }
        }
    }

    /// The files and folders under `folder`, by their paths in it.
    fn files(folder: &Path) -> Vec<String> {
        fn walk(folder: &Path, prefix: &str, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(folder).unwrap() {
                let path = entry.unwrap().path();
                let name = format!("{prefix}{}", path.file_name().unwrap().to_string_lossy());
                match path.is_dir() {
                    // git keeps no empty folder, so the fixtures cannot hold
                    // the empty `fail` the engine always makes; it is
                    // checked by itself.
                    true if std::fs::read_dir(&path).unwrap().next().is_none() => {}
                    true => {
                        out.push(format!("{name}/"));
                        walk(&path, &format!("{name}/"), out);
                    }
                    false => out.push(name),
                }
            }
        }
        let mut out = Vec::new();
        walk(folder, "", &mut out);
        out.sort();
        out
    }

    /// Blanks what a subscription keeps enciphered, its password and its
    /// connection string: the dictionary the original makes has a key of its
    /// own, drawn at random, where tflip's keeps the source's.
    fn mask_subscription_secrets(mut bytes: Vec<u8>) -> Vec<u8> {
        let data = u32::from_le_bytes(bytes[0x20..0x24].try_into().unwrap()) as usize;
        let count = u32::from_le_bytes(bytes[0x18..0x1c].try_into().unwrap()) as usize;
        const RECORD: usize = 524;
        for record in 0..count {
            let base = data + record * RECORD;
            if !bytes[base + 13..base + 23].starts_with(b"Subscriptn") {
                continue;
            }
            let (mut at, mut slot) = (base + 223, 0usize);
            while at + 2 <= base + 223 + 275 {
                let len = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
                if len == 0xffff {
                    (at, slot) = (at + 2, slot + 1);
                    continue;
                }
                let len = len as usize;
                if matches!(slot, 2 | 9) {
                    bytes[at + 2..at + 2 + len].fill(0);
                }
                (at, slot) = (at + 2 + len, slot + 1);
            }
        }
        bytes
    }

    /// A file as the fixtures compare it: a dictionary without the database's
    /// login secrets, which the engine salts anew on every write, a log
    /// without its times and
    /// the folder it names.
    fn comparable(name: &str, bytes: Vec<u8>, folder: &Path, stopped: bool) -> Vec<u8> {
        match name.rsplit('.').next() {
            // A subscription's queue is a table of the dictionary's, whose
            // file is named `q.add` in the scenarios.
            Some("add") if name.rsplit('/').next().is_some_and(|f| f.starts_with('q')) => bytes,
            // The dictionary a stopped deployment leaves has the administrator
            // password the original made for the occasion, in a secret
            // longer than a made one.
            Some("add") if stopped => mask_login(bytes),
            Some("add") => mask_subscription_secrets(mask_public_rights(mask_db_secret(bytes))),
            Some("log") => {
                let text = String::from_utf8(bytes).unwrap();
                let shown = shown(&folder.to_string_lossy());
                text.lines()
                    .map(|line| match line.as_bytes().get(2) {
                        Some(b':') => format!("00:00:00{}\r\n", &line[8..]),
                        _ => format!("{line}\r\n"),
                    })
                    .map(|line| line.replace(&shown, "{D}"))
                    .collect::<String>()
                    .into_bytes()
            }
            // A table the deployment makes carries the day it ran in its
            // header, so the fixture would only match on the day it was probed.
            Some("dbf") => {
                let mut bytes = bytes;
                if bytes.len() >= 4 {
                    bytes[1..4].fill(0);
                }
                bytes
            }
            _ => bytes,
        }
    }

    /// The scenarios of `fixtures/dictionary/deploy` the deployment cannot
    /// do as the original does yet: a field's validation message, which the
    /// original never keeps, while it drops other properties of the field by
    /// a rule not worked out (`w_*`), and the move of the records that break
    /// the constraints of a table with a memo field (`d_*memo*`).
    const NOT_DONE: &[&str] = &[
        "w_msg",
        "w_min",
        "w_max",
        "w_def",
        "w_null",
        "w_all",
        "w_dmin",
        "w_dmax",
        "w_mm",
        "w_allc",
        "w_alln",
        "w_allname",
        "w_allboth",
        "w_two",
        "w_cmt",
        "d_cdx",
        "d_cdxmax",
        "d_ntx",
        "d_vfp",
        "d_vfp2",
        "d_cdxplain",
        "d_cdxfail",
        "d_dbfmemo",
        "d_memo",
        "d_memov",
        "d_vfpmemov",
    ];

    /// What [`NOT_DONE`] lists is refused before anything is written.
    #[test]
    fn refuses_what_is_not_done() {
        let session = Session::new();
        let results = std::fs::read_to_string(fixtures().join("results.txt")).unwrap();
        let work = tflip_storage::testing::temp_root()
            .join(format!("tflip-deploy-notdone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        for name in NOT_DONE {
            let line = results
                .lines()
                .find(|line| line.split('|').nth(1) == Some(name))
                .unwrap();
            let validate = line.split('|').nth(6).unwrap();
            let folder = work.join(name);
            copy_all(&fixtures().join(name).join("before"), &folder);
            // Git keeps no empty folder.
            std::fs::create_dir_all(folder.join("D")).unwrap();
            let before = files(&folder.join("D"));
            let outcome = deploy_database_api(
                &session,
                &folder.join("D").to_string_lossy(),
                &folder.join("S").join("newdb.add").to_string_lossy(),
                "",
                1,
                validate.parse().unwrap(),
                0,
            );
            assert!(matches!(outcome, Err(Error::NotImplemented)), "{name}");
            assert_eq!(files(&folder.join("D")), before, "{name}");
        }
        let _ = std::fs::remove_dir_all(&work);
    }

    /// What the original engine did, scenario by scenario
    /// (`tools/ace/dd/deploy.c`): the result of the call and the files it
    /// leaves in the destination and the source.
    #[test]
    fn deploys_like_the_engine() {
        let session = Session::new();
        let results = std::fs::read_to_string(fixtures().join("results.txt")).unwrap();
        let work = tflip_storage::testing::temp_root()
            .join(format!("tflip-deploy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        let mut failures = Vec::new();
        for line in results.lines() {
            let fields: Vec<&str> = line.splitn(10, '|').collect();
            if fields.get(1).is_some_and(|name| NOT_DONE.contains(name)) {
                continue;
            }
            let [
                _,
                name,
                destination,
                source,
                password,
                types,
                validate,
                backup,
                rc,
                text,
            ] = fields[..]
            else {
                panic!("{line}");
            };
            let root = fixtures().join(name);
            let folder = work.join(name);
            let stopped = matches!(name, "drop_index" | "dbf_extra_index");
            copy_all(&root.join("before"), &folder);
            // Git keeps no empty folder, and a destination may start empty.
            std::fs::create_dir_all(folder.join(destination)).unwrap();
            let outcome = deploy_database_api(
                &session,
                &folder.join(destination).to_string_lossy(),
                &folder.join(source).to_string_lossy(),
                password,
                types.parse().unwrap(),
                validate.parse().unwrap(),
                backup.parse().unwrap(),
            );
            let (code, message) = match outcome {
                Ok(()) => (0, String::new()),
                Err(Error::Sql(sql)) => (sql.native, sql.message),
                Err(error) => (error.to_ace().code, String::new()),
            };
            if code.to_string() != rc {
                failures.push(format!("{name}: result {code}, not {rc} ({message})"));
                continue;
            }
            let message = message.strip_prefix("[ASA] ").unwrap_or(&message);
            if !message.is_empty() && message != text {
                failures.push(format!("{name}: error {message:?}, not {text:?}"));
            }

            // The destination is what the original left, file for file.
            let want = root.join("after/D");
            let ours = folder.join("D");
            if want.join("adsdeploy.log").exists() && !ours.join("fail").is_dir() {
                failures.push(format!("{name}: no fail folder in D"));
            }
            if files(&ours) != files(&want) {
                failures.push(format!(
                    "{name}: files of D {:?}, not {:?}",
                    files(&ours),
                    files(&want)
                ));
                continue;
            }
            for file in files(&want).iter().filter(|f| !f.ends_with('/')) {
                let wanted = std::fs::read(want.join(file)).unwrap();
                let made = std::fs::read(ours.join(file)).unwrap();
                if comparable(file, made, &ours, stopped)
                    != comparable(file, wanted, &ours, stopped)
                {
                    failures.push(format!("{name}: D/{file}"));
                }
            }
            // The rights granted to `DB:Public` have a random part, masked above:
            // what they decipher to is the original's.
            if want.join("newdb.add").exists() {
                assert_eq!(
                    public_rights_of(&ours.join("newdb.add")),
                    public_rights_of(&want.join("newdb.add")),
                    "{name}: the rights of DB:Public"
                );
            }
            // The secret is masked above, so the password it holds is looked at
            // by itself: the deployed dictionary has the source's.
            if code == 0 && !password.is_empty() {
                let deployed =
                    tflip_storage::dd::Dictionary::open(&ours.join("newdb.add")).unwrap();
                assert!(
                    deployed.dictionary_key(password.as_bytes()).is_some(),
                    "{name}: the password"
                );
            }
            // The source's dictionary files, put aside as `adsbackup`.
            let before = root.join("before/S");
            let source_folder = folder.join("S");
            for file in files(&before).iter().filter(|f| !f.ends_with('/')) {
                let kept = std::fs::read(before.join(file)).unwrap();
                let aside = file.replace("newdb.", "adsbackup.");
                let Some(now) = [file, &aside]
                    .iter()
                    .find_map(|f| std::fs::read(source_folder.join(f)).ok())
                else {
                    failures.push(format!("{name}: S/{file} is gone"));
                    continue;
                };
                if comparable(file, now, &ours, false) != comparable(file, kept, &ours, false) {
                    failures.push(format!("{name}: S/{file}"));
                }
            }
        }
        let _ = std::fs::remove_dir_all(&work);
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// What a deployment leaves alone it cannot do as the original does,
    /// before it writes anything: a field's message, which the original keeps
    /// only sometimes, here.
    #[test]
    fn refuses_what_it_cannot_do() {
        let session = Session::new();
        let work = tflip_storage::testing::temp_root()
            .join(format!("tflip-deploy-cannot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        let (source, dest) = (work.join("S"), work.join("D"));
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        let connect = test_support::build_ops_base(&session, &source);
        let call = || {
            deploy_database_api(
                &session,
                &dest.to_string_lossy(),
                &source.join("db.add").to_string_lossy(),
                "",
                1,
                0,
                0,
            )
        };
        set_field_property_api(&session, connect, "t", "id", 304, b"too low\0").unwrap();
        connection::disconnect(&session, connect).unwrap();
        assert!(matches!(call(), Err(Error::NotImplemented)));
        assert!(files(&dest).is_empty(), "{:?}", files(&dest));
        let _ = std::fs::remove_dir_all(&work);
    }

    /// What the original answers a call that cannot go on, option by option
    /// and in the order it looks at them (`tools/ace/dd/deploy.c`'s `types*`,
    /// `validate*`, `backup2`, `bad_*`, `no_*` and `src_pw_*` scenarios).
    #[test]
    fn refuses_like_the_engine() {
        let session = Session::new();
        let work = tflip_storage::testing::temp_root()
            .join(format!("tflip-deploy-refuse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        copy_all(&fixtures().join("same_backup/before"), &work);
        let (dest, add) = (work.join("D"), work.join("S/newdb.add"));
        let call = |dest: &Path, add: &Path, password: &str, options: (u16, u16, u16)| {
            let outcome = deploy_database_api(
                &session,
                &dest.to_string_lossy(),
                &add.to_string_lossy(),
                password,
                options.0,
                options.1,
                options.2,
            );
            match outcome {
                Ok(()) => 0,
                Err(Error::Sql(sql)) => sql.native,
                Err(error) => error.to_ace().code,
            }
        };
        let missing = work.join("missing");
        // The options come first, the validation's before the server types'
        // and these before the backup's, then the paths.
        for (options, code) in [
            ((0, 0, 0), 5019),
            ((3, 0, 0), 5019),
            ((8, 0, 0), 5019),
            ((9, 0, 9), 5019),
            ((4, 0, 0), 5033),
            ((1, 1, 0), 5019),
            ((1, 3, 0), 5019),
            ((9, 9, 9), 5019),
            ((1, 0, 2), 5019),
            ((1, 9, 9), 5019),
        ] {
            assert_eq!(call(&missing, &missing, "", options), code, "{options:?}");
        }
        assert_eq!(call(&dest, &add, "", (1, 1, 9)), 5019);
        // The folder and the dictionary are to be there (5020), and the
        // password the dictionary's own (7078).
        assert_eq!(call(&missing, &add, "", (1, 0, 0)), 5020);
        assert_eq!(call(&dest, &missing, "", (1, 0, 0)), 5020);
        assert_eq!(call(&dest, &add, "secret", (1, 0, 0)), 7078);
        // Nothing was written by any of them.
        assert_eq!(
            files(&dest),
            files(&fixtures().join("same_backup/before/D"))
        );
        let _ = std::fs::remove_dir_all(&work);
    }
}
