//! The system procedures that work on one table: `sp_PackTable`,
//! `sp_ZapTable`, `sp_Reindex`, `sp_EncryptTable`, `sp_DecryptTable`,
//! `sp_GetTableEncryptionType`, `sp_GetTableSizeInfo`, `sp_GetRecordCRC`, and
//! the `…Online` forms, which a local server refuses. Their results and
//! errors are as the original engine's gives them (`fixtures/sql/spt.json`,
//! `sptdbf.json`, `sptdd.json`).

use std::path::Path;

use tflip_sql::ast::Execute;
use tflip_sql::{SqlError, V};
use tflip_storage::dbf::{self, Flavour};
use tflip_storage::field::FieldType;
use tflip_storage::memo::{MemoFile, MemoKind};
use tflip_storage::table::Format;
use tflip_storage::value::Value;

mod online;

use super::Results;
use super::args::{Kind, Spec, check_arguments, column, encryption_name};
use crate::ace_error;
use crate::ace_error::error_text;
use crate::handle::Handle;
use crate::statement::Settings;
use crate::table::{self, options};
use crate::{Error, Session, connection, index};

const TABLE: (&str, Kind) = ("TableName", Kind::Text);
const OPTIONS: (&str, Kind) = ("Options", Kind::Options);
const PASSWORD: (&str, Kind) = ("Password", Kind::Text);

const SPECS: [Spec; 12] = [
    Spec {
        name: "SP_PACKTABLE",
        params: &[TABLE, ("MemoBlockSize", Kind::Integer)],
        required: 1,
        results: false,
    },
    Spec {
        name: "SP_ZAPTABLE",
        params: &[TABLE],
        required: 1,
        results: false,
    },
    Spec {
        name: "SP_REINDEX",
        params: &[TABLE, ("PageSize", Kind::Integer)],
        required: 2,
        results: false,
    },
    Spec {
        name: "SP_ENCRYPTTABLE",
        params: &[TABLE, PASSWORD],
        required: 1,
        results: false,
    },
    Spec {
        name: "SP_DECRYPTTABLE",
        params: &[TABLE, PASSWORD],
        required: 1,
        results: false,
    },
    Spec {
        name: "SP_GETTABLEENCRYPTIONTYPE",
        params: &[TABLE],
        required: 1,
        results: true,
    },
    Spec {
        name: "SP_GETTABLESIZEINFO",
        params: &[TABLE],
        required: 1,
        results: true,
    },
    Spec {
        name: "SP_GETRECORDCRC",
        params: &[TABLE, ("recordnumber", Kind::Integer)],
        required: 2,
        results: true,
    },
    Spec {
        name: "SP_PACKTABLEONLINE",
        params: &[TABLE, OPTIONS],
        required: 1,
        results: false,
    },
    Spec {
        name: "SP_REINDEXONLINE",
        params: &[TABLE, OPTIONS],
        required: 1,
        results: false,
    },
    Spec {
        name: "SP_IGNORETABLETRANSACTIONS",
        params: &[TABLE, ("IgnoreTrans", Kind::Flag)],
        required: 1,
        results: false,
    },
    Spec {
        name: "SP_PACKALLTABLESONLINE",
        params: &[OPTIONS],
        required: 0,
        results: false,
    },
];

fn spec(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

/// The parameters of the procedure `name`, as fields (none for one that is not
/// one of these).
pub(crate) fn inputs(name: &str) -> Vec<tflip_storage::field::Field> {
    spec(name).map_or_else(Vec::new, super::args::inputs)
}

/// Whether `name` is one of these procedures.
pub(crate) fn handles(name: &str) -> bool {
    spec(name).is_some()
}

/// Whether the procedure `name` has a result set (true for any that is not
/// one of these).
pub(crate) fn has_results(name: &str) -> bool {
    spec(name).is_none_or(|s| s.results)
}

/// Runs the procedure `execute` calls with `args`.
pub(crate) fn run(
    session: &Session,
    statement: Handle,
    connection: Handle,
    execute: &Execute,
    args: &[V],
) -> Result<Option<Results>, Error> {
    let spec = spec(&execute.name).ok_or(Error::NotImplemented)?;
    let args = check_arguments(spec, execute, args)?;
    // A statement that is only described has no table settings of its own.
    let settings = crate::statement::settings_of(session, statement).unwrap_or_default();
    let target = Target {
        session,
        connection,
        settings: &settings,
    };
    let name = || args.text(0).ok_or_else(|| ace_error::sql(5020, ""));
    match spec.name {
        "SP_PACKTABLEONLINE" | "SP_REINDEXONLINE" | "SP_PACKALLTABLESONLINE"
            if session.shares.is_some() =>
        {
            online::run(&target, spec.name, &args)?;
            Ok(None)
        }
        // A local server cannot do them.
        "SP_PACKTABLEONLINE" | "SP_REINDEXONLINE" | "SP_PACKALLTABLESONLINE" => {
            let procedure = &execute.name;
            let instead = match spec.name {
                "SP_PACKTABLEONLINE" => "  Use sp_PackTable instead.",
                "SP_REINDEXONLINE" => "  Use sp_Reindex instead.",
                _ => "",
            };
            Err(ace_error::sql(
                5042,
                &format!(
                    "{procedure} cannot be called with Advantage Local Server connections.{instead}"
                ),
            ))
        }
        "SP_PACKTABLE" => {
            let name = name()?;
            let block_size = match args.integer(1) {
                Some(size) => memo_block_size(&target, &name, size)?,
                None => None,
            };
            target.maintain(&name, |handle| {
                index::pack_blocks(session, handle, false, block_size)
            })?;
            if let Some(size) = block_size {
                target.record_in_dictionary(&name, Recorded::MemoBlockSize(size))?;
            }
            Ok(None)
        }
        "SP_ZAPTABLE" => {
            let name = name()?;
            target.maintain(&name, |handle| {
                index::pack_blocks(session, handle, true, None)
            })?;
            Ok(None)
        }
        "SP_REINDEX" => {
            let name = name()?;
            let page_size = match args.integer(1).unwrap_or(0) {
                0 => None,
                512..=8192 => args.integer(1).map(|size| size as u16),
                _ => {
                    return Err(ace_error::sql(
                        5019,
                        "Parameter 2.  Page size is outside valid range.",
                    ));
                }
            };
            target.maintain(&name, |handle| {
                index::reindex_pages(session, handle, page_size)
            })?;
            if let Some(size) = page_size {
                target.record_in_dictionary(&name, Recorded::PageSize(size))?;
            }
            Ok(None)
        }
        // The table's header says its changes are not in transactions; a
        // dictionary's table is told so in the dictionary too.
        "SP_IGNORETABLETRANSACTIONS" => {
            // Left out it makes the table transaction-free, NULL does not.
            let free = args.values.len() < 2 || args.integer(1).is_some_and(|n| n != 0);
            target.ignore_transactions(&args.text(0).unwrap_or_default(), free)?;
            Ok(None)
        }
        "SP_ENCRYPTTABLE" => {
            target.encrypt(&name()?, args.text(1))?;
            Ok(None)
        }
        "SP_DECRYPTTABLE" => {
            target.decrypt(&name()?, args.text(1))?;
            Ok(None)
        }
        "SP_GETTABLEENCRYPTIONTYPE" => target.encryption_type(&name()?).map(Some),
        "SP_GETTABLESIZEINFO" => target.size_info(&name()?).map(Some),
        _ => {
            let record = args.integer(1).unwrap_or(0);
            target.record_crc(&name()?, record).map(Some)
        }
    }
}

/// The memo block size `size` the SQL engine gives to a pack of the table
/// `name`: none for 0 (or 65,536, which it cuts to 0). Its limits differ by
/// table type, and a FoxPro or Visual FoxPro table's sizes up to 32 count
/// 512-byte units, as in `CREATE TABLE` (`fixtures/sql/sptdbf.json`).
fn memo_block_size(target: &Target, name: &str, size: i64) -> Result<Option<u16>, Error> {
    let invalid = || ace_error::sql(5058, "AdsPackTable");
    // Cut to 16 bits, as the original engine does.
    let size = size as u16;
    if size == 0 {
        return Ok(None);
    }
    let flavour = target.table_type_of(name)?;
    let bytes = match flavour {
        // A Clipper memo file's blocks are 512 bytes whatever the size.
        Some(Flavour::Clipper) if size <= 1024 => return Ok(None),
        Some(Flavour::Clipper) => return Err(invalid()),
        Some(_) => match size {
            1..=32 => size * 512,
            33..=1024 => size,
            _ => return Err(invalid()),
        },
        None => match size {
            8..=32768 => size,
            _ => return Err(invalid()),
        },
    };
    Ok(Some(bytes))
}

/// What a dictionary keeps of a table's files, which a pack or reindex changes.
enum Recorded {
    /// The table's memo block size, in bytes.
    MemoBlockSize(u16),
    /// The page size of the table's ADI files.
    PageSize(u16),
    /// Whether the table is transaction-free.
    TransactionFree(bool),
}

/// Where a procedure's table is looked for.
struct Target<'a> {
    session: &'a Session,
    connection: Handle,
    settings: &'a Settings,
}

impl Target<'_> {
    /// Whether the connection is to a data dictionary, whose tables the
    /// procedures name by their names there.
    fn dictionary(&self) -> bool {
        connection::get(self.session, self.connection).is_ok_and(|owner| owner.dictionary)
    }

    /// 5132 for a table the connection's dictionary does not have.
    fn check_in_dictionary(&self, name: &str) -> Result<(), Error> {
        if !self.dictionary() {
            return Ok(());
        }
        let owner = connection::get(self.session, self.connection)?;
        match owner.with_database_opt(|database| database.table(name)) {
            Some(Some(_)) => Ok(()),
            _ => Err(ace_error::sql(
                5132,
                &format!("\"{name}\" is not a table or view in the data dictionary."),
            )),
        }
    }

    /// Opens the table `name` with the open `options`, and `password` if
    /// it has one.
    fn open(&self, name: &str, options: u32, password: &str) -> Result<Handle, Error> {
        let name = name.trim_end();
        if name.is_empty() {
            return Err(ace_error::sql(5020, ""));
        }
        self.check_in_dictionary(name)?;
        let mut settings = self.settings.table(options);
        settings.password = password.to_owned();
        table::open(self.session, self.connection, name, settings.clone()).map_err(|error| {
            let error = match error {
                Error::LockFailed | Error::FileInUse => Error::FileInUse,
                error => error,
            };
            // A dictionary's table is refused without its file named, as the
            // statements refuse it (`fixtures/events/events_online_procs.json`).
            if error == Error::FileInUse && self.dictionary() {
                return ace_error::sql_error(error);
            }
            ace_error::sql_error(table::with_file(
                self.session,
                self.connection,
                name,
                settings,
                error,
            ))
        })
    }

    /// Runs `work` on the table `name`, opened exclusively and closed again.
    fn exclusively<T>(
        &self,
        name: &str,
        password: &str,
        work: impl FnOnce(Handle) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let handle = self.open(name, options::EXCLUSIVE, password)?;
        let result = work(handle);
        let _ = table::close(self.session, handle);
        result.map_err(ace_error::sql_error)
    }

    /// [`Target::maintain`] for an online procedure: the exclusive open
    /// waits, as `wait` says, for the other handles on the table to go, and
    /// `timed_out` is the error of the wait ending first.
    fn exclusively_waiting(
        &self,
        name: &str,
        wait: online::Wait,
        timed_out: Error,
        work: impl FnOnce(Handle) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let handle = loop {
            match self.open(name, options::EXCLUSIVE, "") {
                Err(Error::Sql(error)) if error.native == 7008 => {
                    if wait.over() {
                        return Err(timed_out);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                opened => break opened?,
            }
        };
        let result = (|| {
            let table = table::get(self.session, handle)?;
            if table.structure.header.encrypted() && !table.encryption_enabled() {
                return Err(Error::Sql(SqlError::ace(
                    5098,
                    error_text(5098).unwrap_or_default(),
                    "Password is required to open encrypted table!",
                )));
            }
            work(handle)
        })();
        let _ = table::close(self.session, handle);
        result.map_err(ace_error::sql_error)
    }

    /// The tables `sp_PackAllTablesOnline` packs: a dictionary's, or the
    /// folder's own ADT tables.
    fn all_table_names(&self) -> Result<Vec<String>, Error> {
        let owner = connection::get(self.session, self.connection)?;
        if owner.dictionary {
            return Ok(owner
                .with_database_opt(|database| database.table_names())
                .unwrap_or_default());
        }
        let mut names: Vec<String> = std::fs::read_dir(owner.folder())
            .map_err(|_| Error::FileNotFound)?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let adt = path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("adt"));
                let stem = path.file_stem()?.to_str()?.to_owned();
                (adt && path.is_file()).then_some(stem)
            })
            .collect();
        names.sort_by_key(|name| name.to_lowercase());
        Ok(names)
    }

    /// The memo block size an online pack gives, as [`memo_block_size`] reads
    /// it, with the error the original words without a call's name.
    fn memo_block_size_online(&self, name: &str, size: i64) -> Result<Option<u16>, Error> {
        memo_block_size(self, name, size).map_err(|error| match error {
            Error::Sql(sql) if sql.native == 5058 => Error::Sql(SqlError::ace_bare(
                5058,
                error_text(5058).unwrap_or_default(),
            )),
            error => error,
        })
    }

    /// A pack, zap or reindex: refused for an encrypted free table, as the
    /// original refuses it without the password.
    fn maintain(
        &self,
        name: &str,
        work: impl FnOnce(Handle) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.exclusively(name, "", |handle| {
            let table = table::get(self.session, handle)?;
            if table.structure.header.encrypted() && !table.encryption_enabled() {
                return Err(Error::Sql(SqlError::ace(
                    5098,
                    error_text(5098).unwrap_or_default(),
                    "Password is required to open encrypted table!",
                )));
            }
            work(handle)
        })
    }

    /// Writes what `recorded` says into the dictionary the connection is to,
    /// for its table `name` (a free table has none): the original engine keeps
    /// a table's memo block size and its index files' page sizes there
    /// (`fixtures/sql/sptdd`).
    fn record_in_dictionary(&self, name: &str, recorded: Recorded) -> Result<(), Error> {
        use tflip_storage::dd::{self, Dictionary, Kind};
        if !self.dictionary() {
            return Ok(());
        }
        let (owner, add_path) =
            crate::dictionary_write::dictionary_connection(self.session, self.connection)?;
        let Some(Some(bound)) = owner.with_database_opt(|database| database.table(name.trim_end()))
        else {
            return Ok(());
        };
        let dictionary = Dictionary::open(&add_path).map_err(|_| Error::CorruptTable)?;
        let table = dictionary
            .object(bound.object_id)
            .ok_or(Error::CorruptTable)?;
        match recorded {
            Recorded::MemoBlockSize(size) => {
                let name = table.name.clone();
                crate::dictionary_write::set_table_property_api(
                    self.session,
                    self.connection,
                    &name,
                    215,
                    &size.to_le_bytes(),
                )
            }
            Recorded::TransactionFree(free) => {
                let name = table.name.clone();
                crate::dictionary_write::set_table_property_api(
                    self.session,
                    self.connection,
                    &name,
                    220,
                    &[u8::from(free), 0],
                )
            }
            Recorded::PageSize(size) => {
                let files: Vec<u32> = dictionary
                    .children(table.id, Kind::IndexFile)
                    .into_iter()
                    .filter(|file| {
                        file.text(dd::index_file::RELATIVE_PATH)
                            .is_some_and(|path| path.trim_end_matches('\0').ends_with(".adi"))
                    })
                    .map(|file| file.record())
                    .collect();
                for record in files {
                    crate::dictionary_write::set_object_slot(
                        self.session,
                        &owner,
                        &add_path,
                        record,
                        dd::index_file::PAGE_SIZE,
                        &u32::from(size).to_le_bytes(),
                    )?;
                }
                Ok(())
            }
        }
    }

    /// The DBF flavour of the table `name`, none for an ADT table.
    fn table_type_of(&self, name: &str) -> Result<Option<Flavour>, Error> {
        let handle = self.open(name, 0, "")?;
        let flavour = table::get(self.session, handle).map(|t| t.dbf_flavour());
        let _ = table::close(self.session, handle);
        flavour
    }

    /// `sp_IgnoreTableTransactions`: the table `name` made transaction-free, or
    /// not.
    fn ignore_transactions(&self, name: &str, free: bool) -> Result<(), Error> {
        if name.trim().is_empty() {
            // A free table is looked for by an empty name's file; the
            // dictionary refuses it first.
            if self.dictionary() {
                return Err(ace_error::sql(
                    5021,
                    "Parameter 2. Table name cannot be an empty string.",
                ));
            }
            let folder = connection::get(self.session, self.connection)?.folder();
            let file = folder.join(".adt");
            return Err(ace_error::sql_error(
                Error::FileNotFound.detailed(file.display().to_string()),
            ));
        }
        self.exclusively(name, "", |handle| {
            table::get(self.session, handle)?.set_transaction_free(self.session, free)
        })?;
        match self.dictionary() {
            true => self.record_in_dictionary(name, Recorded::TransactionFree(free)),
            false => Ok(()),
        }
    }

    /// `sp_EncryptTable`: a free table needs the password, a dictionary's
    /// takes the dictionary's, which a call cannot give.
    fn encrypt(&self, name: &str, password: Option<String>) -> Result<(), Error> {
        if self.dictionary() {
            self.check_in_dictionary(name)?;
            if password.is_some() {
                return Err(ace_error::sql(
                    5019,
                    "Table encryption is enabled through a dictionary property for database \
                     tables.",
                ));
            }
            return crate::dictionary_write::set_table_encryption(
                self.session,
                self.connection,
                name.trim_end(),
                true,
                false,
            )
            .map_err(ace_error::sql_error);
        }
        let Some(password) = password else {
            return Err(ace_error::sql(5125, ""));
        };
        self.exclusively(name, "", |handle| {
            let table = table::get(self.session, handle)?;
            let encrypted = table.structure.header.encrypted();
            match table.enable_encryption(password.as_bytes()) {
                Err(Error::InvalidPassword) => Err(Error::Sql(SqlError::ace(
                    5098,
                    error_text(5098).unwrap_or_default(),
                    "The table is encrypted and the given password does not match.",
                ))),
                Err(error) => Err(error),
                Ok(()) if encrypted => Ok(()),
                Ok(()) => table.encrypt_table(),
            }
        })
    }

    /// `sp_DecryptTable`.
    fn decrypt(&self, name: &str, password: Option<String>) -> Result<(), Error> {
        if self.dictionary() {
            self.check_in_dictionary(name)?;
            if password.is_some() {
                return Err(ace_error::sql(
                    5097,
                    "A password should not be given when decrypting dictionary-bound tables.",
                ));
            }
            return crate::dictionary_write::set_table_encryption(
                self.session,
                self.connection,
                name.trim_end(),
                false,
                false,
            )
            .map_err(ace_error::sql_error);
        }
        let Some(password) = password else {
            return Err(ace_error::sql(5125, ""));
        };
        self.exclusively(name, "", |handle| {
            let table = table::get(self.session, handle)?;
            if !table.structure.header.encrypted() {
                return Ok(());
            }
            table.enable_encryption(password.as_bytes())?;
            table.decrypt_table()?;
            table.disable_encryption()
        })
    }

    /// `sp_GetTableEncryptionType`.
    fn encryption_type(&self, name: &str) -> Result<Results, Error> {
        let handle = self.open(name, 0, "")?;
        let table = table::get(self.session, handle);
        let _ = table::close(self.session, handle);
        let header = table
            .map_err(ace_error::sql_error)?
            .structure
            .header
            .clone();
        let kind = match header.encrypted() {
            false => "NONE",
            true => encryption_name(u32::from(header.encryption_type)),
        };
        Ok(Results::new(
            vec![column("EncryptionType", FieldType::Character, 10)],
            vec![vec![Value::Text(kind.to_owned())]],
        ))
    }

    /// `sp_GetTableSizeInfo`.
    fn size_info(&self, name: &str) -> Result<Results, Error> {
        let handle = self.open(name, 0, "")?;
        let table = table::get(self.session, handle);
        let _ = table::close(self.session, handle);
        let table = table.map_err(ace_error::sql_error)?;
        let header = &table.structure.header;
        let (memo_header, memo_block, memo_blocks) = memo_sizes(&table);
        let deleted = match header.format {
            Format::Adt => header.free_count,
            // The original counts none for DBF tables, as its help says.
            Format::Dbf(_) => 0,
        };
        let integer = |name: &str| column(name, FieldType::Integer, 4);
        let numbers = [
            header.data_offset,
            header.record_length,
            table.slot_count(),
            deleted,
            memo_header,
            memo_block,
            memo_blocks,
        ];
        Ok(Results::new(
            vec![
                integer("TableHeaderSize"),
                integer("RecordLength"),
                integer("RawRecordCount"),
                integer("DeletedRecordCount"),
                integer("MemoHeaderSize"),
                integer("MemoBlockSize"),
                integer("MemoBlockCount"),
            ],
            vec![
                numbers
                    .iter()
                    .map(|&n| Value::Integer(i128::from(n as i32)))
                    .collect(),
            ],
        ))
    }

    /// `sp_GetRecordCRC`. The table is named by its path, which a free
    /// connection takes relative to the process, not its own folder.
    fn record_crc(&self, name: &str, record: i64) -> Result<Results, Error> {
        let path;
        let mut name = name.trim_end();
        if !self.dictionary() && !Path::new(name).is_absolute() {
            path = std::env::current_dir()
                .map(|dir| dir.join(name))
                .map_err(|_| ace_error::sql(5020, ""))?;
            name = path.to_str().ok_or_else(|| ace_error::sql(5020, ""))?;
        }
        let handle = self.open(name, 0, "")?;
        let result = (|| {
            let table = table::get(self.session, handle)?;
            // An empty table has a blank record to give a checksum of, whichever
            // record is asked for.
            if table.slot_count() == 0 {
                // (its memo values are not there to read)
                return match table.fields().iter().any(|f| f.is_memo()) {
                    true => Err(Error::NoCurrentRecord),
                    false => Ok(table.blank_record_crc()),
                };
            }
            table
                .goto_record(u32::try_from(record).unwrap_or(0))
                .map_err(|error| match error {
                    Error::InvalidRecordNumber => {
                        error.detailed("You cannot go to deleted records in ADT tables.")
                    }
                    error => error,
                })?;
            table.record_crc(false)
        })();
        let _ = table::close(self.session, handle);
        let crc = result.map_err(ace_error::sql_error)?;
        Ok(Results::new(
            vec![
                column("CRCInteger", FieldType::Integer, 4),
                column("CRCString", FieldType::Character, 10),
            ],
            vec![vec![
                Value::Integer(i128::from(crc as i32)),
                Value::Text(crc.to_string()),
            ]],
        ))
    }
}

/// The header size, block size and block count of a table's memo file, all 0
/// for a table without one. The header is as much of the file as there is up
/// to the block-aligned start of the first value, and the count how many
/// blocks the file has past it (probed on files of every block size,
/// `fixtures/sql/sptdbf`, `spt`).
fn memo_sizes(table: &table::Table) -> (u32, u32, u32) {
    let header = &table.structure.header;
    let (path, kind) = match header.format {
        Format::Adt => (MemoFile::path_for(&table.path), MemoKind::Adm),
        Format::Dbf(Flavour::Clipper) => {
            (dbf::memo_path(&table.path, MemoKind::Dbt), MemoKind::Dbt)
        }
        Format::Dbf(_) => (dbf::memo_path(&table.path, MemoKind::Fpt), MemoKind::Fpt),
    };
    let Ok(length) = std::fs::metadata(&path).map(|m| m.len()) else {
        return (0, 0, 0);
    };
    let Some(block) = table.memo_block_size() else {
        return (0, 0, 0);
    };
    let block = u64::from(block.max(1));
    let (header_size, blocks) = match kind {
        MemoKind::Adm => (
            length.min(4096),
            length.div_ceil(block).saturating_sub(4096 / block),
        ),
        _ => (512, length.saturating_sub(512).div_ceil(block)),
    };
    (header_size as u32, block as u32, blocks as u32)
}
