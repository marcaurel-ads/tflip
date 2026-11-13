//! The result set of the backup and restore procedures: the rows the
//! original's server answers (`fixtures/backup/README.md`), each as a
//! [`Notice`] that knows its severity, code, texts and the line of the
//! original's source it reports. A lane that adds a row adds a variant here.

use tflip_storage::build::Column;
use tflip_storage::field::FieldType;
use tflip_storage::value::Value;

use crate::ace_error::error_text;
use crate::procedure::args::column;

/// The file every row names as its source, as the original's cursor pads it.
const SOURCE_FILE: &str = "..\\ACE\\aceunpub.cpp";

/// The columns of the result set.
pub(crate) fn columns() -> Vec<Column> {
    vec![
        column("Severity", FieldType::Integer, 4),
        column("Error Code", FieldType::Integer, 4),
        column("Error Message", FieldType::Memo, 9),
        column("Table Name", FieldType::Memo, 9),
        column("Additional Info", FieldType::Memo, 9),
        column("Source File", FieldType::Character, 32),
        column("Source Line", FieldType::Integer, 4),
    ]
}

/// One row of the result set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub severity: i32,
    pub code: u32,
    pub message: Option<String>,
    pub table: Option<String>,
    pub info: String,
    pub line: u32,
}

impl Row {
    pub(crate) fn values(&self) -> Vec<Value> {
        let text = |text: &Option<String>| text.clone().map_or(Value::Null, Value::Text);
        vec![
            Value::Integer(i128::from(self.severity)),
            Value::Integer(i128::from(self.code)),
            text(&self.message),
            text(&self.table),
            // A row with nothing to add holds NULL.
            match self.info.is_empty() {
                true => Value::Null,
                false => Value::Text(self.info.clone()),
            },
            Value::Text(SOURCE_FILE.to_owned()),
            Value::Integer(i128::from(self.line)),
        ]
    }
}

/// `Error <code>:  <the original's text>`, then `detail` after two spaces: how
/// a row words the error it reports.
pub(crate) fn error_message(code: u32, detail: Option<&str>) -> String {
    let text = error_text(code).unwrap_or_default();
    match detail {
        Some(detail) => format!("Error {code}:  {text}  {detail}"),
        None => format!("Error {code}:  {text}"),
    }
}

/// What a row reports. The paths are as the call gave them, with the
/// separators the original's server puts between them (`\`).
#[derive(Debug, Clone)]
pub(crate) enum Notice {
    /// An include name that is no table of the dictionary (backup; the other
    /// tables are backed up).
    UnknownTable { name: String },
    /// A table that could not be opened for the backup: a dictionary's table
    /// is named, a free table by its path.
    OpenForBackup {
        table: String,
        code: u32,
        detail: Option<String>,
        free: bool,
    },
    /// No file of the source matches (free tables), with the trailing `\`.
    NoSourceTables { path: String },
    /// The restore of free tables finds no table in the image's folder.
    NoTablesToRestore { path: String },
    /// A file the call lists twice, which the backup of a table that is not
    /// an ADT one refuses the second time.
    ListedTwice { table: String },
    /// A table that is encrypted and for which the call has no password:
    /// `path` is where its image would be, `listed` whether the call gave
    /// passwords for other tables.
    NoPassword { path: String, listed: bool },
    /// A table whose data could not be backed up (5097: no password, 7060: a
    /// wrong one), by its file name.
    BackingUp { table: String, code: u32 },
    /// A table the restore copied and could not index again: `detail` is the
    /// text 5097 gets, and 7060 is the password the table refused.
    Reindex {
        table: String,
        code: u32,
        detail: Option<String>,
    },
    /// A table of the image the restore could not copy: `code` is the
    /// system's (80 the file is there and may not be overwritten, 32 it is
    /// the image's own, 2 it is not there).
    CopyFree { table: String, code: u32 },
    /// A differential backup of a table that has no image yet.
    NewImage { table: String },
    /// A differential backup that writes a table's image afresh: its fields
    /// differ from the image's (`structure`), or its stamp is not the
    /// image's.
    Reinitialised { table: String, structure: bool },
    /// The archive file could not be made (`ArchiveFile=` with no name): the
    /// destination folder with its trailing `\`; the original reports it with
    /// two codes, 3 and 7040, from two places.
    ArchiveFile { dest: String, code: u32 },
    /// A file of the image whose name is too long for the archive's header
    /// (`path` as the original words it: the destination, a `\`, the name).
    ArchiveNameTooLong { path: String },
    /// The backup archive could not be made, which follows the rows of the
    /// archive file; `table` names the archive (as the call gave it) if a
    /// file name was too long for it, which is the other code.
    ArchiveBackup { table: Option<String> },
    /// A table, or the dictionary file when `dictionary`, that another
    /// backup has taken.
    InAnotherBackup { table: String, dictionary: bool },
    /// The restore could not put the dictionary where it was asked to
    /// (`code` 32: it is open; 3: the folder cannot be made).
    DictionaryMetadata { source: String, code: u32 },
    /// A table the restore could not open or create at its destination.
    OpenOrCreate { table: String, code: u32 },
    /// A table of the dictionary whose file the image lacks or cannot open.
    OpenSource {
        table: String,
        code: u32,
        detail: String,
    },
}

impl Notice {
    pub(crate) fn row(&self) -> Row {
        match self {
            Notice::UnknownTable { name } => Row {
                severity: 7,
                code: 5132,
                message: Some(error_message(
                    5132,
                    Some(&format!(
                        "\"{name}\" is not a table or view in the data dictionary."
                    )),
                )),
                table: Some(name.clone()),
                info: "Error opening for backup.".to_owned(),
                line: 9680,
            },
            Notice::OpenForBackup {
                table,
                code,
                detail,
                free,
            } => Row {
                severity: 7,
                code: *code,
                message: Some(error_message(*code, detail.as_deref())),
                table: Some(table.clone()),
                info: "Error opening for backup.".to_owned(),
                line: if *free { 9695 } else { 9680 },
            },
            Notice::NoSourceTables { path } => Row {
                severity: 7,
                code: 0,
                message: None,
                table: Some(path.clone()),
                info: "No source tables found. Verify source path is valid.".to_owned(),
                line: 9663,
            },
            Notice::NoTablesToRestore { path } => Row {
                severity: 7,
                code: 0,
                message: None,
                table: Some(path.clone()),
                info: "No source tables found. Verify source path is valid.".to_owned(),
                line: 9990,
            },
            Notice::ListedTwice { table } => Row {
                severity: 10,
                code: 7131,
                message: None,
                table: Some(table.clone()),
                info: "Error initializing table for hot backup. See error code for more \
                       details."
                    .to_owned(),
                line: 8009,
            },
            Notice::NoPassword { path, listed } => Row {
                severity: 5,
                code: 5097,
                message: Some(error_message(
                    5097,
                    Some(match listed {
                        false => "Encrypted table encountered but no passwords were supplied.",
                        true => {
                            "Encrypted table encountered but no password was passed in for \
                             this table."
                        }
                    }),
                )),
                table: Some(path.clone()),
                info: String::new(),
                line: if *listed { 6854 } else { 6842 },
            },
            Notice::BackingUp { table, code } => Row {
                severity: 7,
                code: *code,
                message: (*code != 5097).then(|| error_message(*code, None)),
                table: Some(table.clone()),
                info: "Error backing up table data.".to_owned(),
                line: 9817,
            },
            Notice::Reindex {
                table,
                code,
                detail,
            } => match *code {
                7060 => Row {
                    severity: 10,
                    code: 7060,
                    message: None,
                    table: Some(table.clone()),
                    info: "Unable to reindex table after it was restored. Error setting \
                           encryption password."
                        .to_owned(),
                    line: 10183,
                },
                code => Row {
                    severity: 10,
                    code,
                    message: Some(error_message(code, detail.as_deref())),
                    table: Some(table.clone()),
                    info: "Unable to reindex table after it was restored.".to_owned(),
                    line: 10157,
                },
            },
            Notice::CopyFree { table, code } => Row {
                severity: 10,
                code: *code,
                message: None,
                table: Some(table.clone()),
                info: "Error copying free table during restore operation.".to_owned(),
                line: 10033,
            },
            Notice::NewImage { table } => Row {
                severity: 5,
                code: 0,
                message: None,
                table: Some(table.clone()),
                info: "Error opening backup image of table for differential backup. Assuming \
                       this is a new table and creating a new table in the backup image \
                       automatically."
                    .to_owned(),
                line: 6660,
            },
            Notice::Reinitialised { table, structure } => Row {
                severity: 5,
                code: 0,
                message: None,
                table: Some(table.clone()),
                info: match structure {
                    true => {
                        "Table structure does not match the backup image table structure. \
                             Re-initializing the differential backup for this table."
                    }
                    false => {
                        "The differential backup times do not match. Re-initializing the \
                              differential backup for this table."
                    }
                }
                .to_owned(),
                line: if *structure { 6693 } else { 6709 },
            },
            Notice::ArchiveFile { dest, code } => Row {
                severity: 10,
                code: *code,
                message: None,
                table: Some(format!("{}\\", dest.trim_end_matches(['\\', '/']))),
                info: "Error creating archive file.".to_owned(),
                line: if *code == 3 { 8927 } else { 8930 },
            },
            Notice::ArchiveNameTooLong { path } => Row {
                severity: 10,
                code: 5050,
                message: None,
                table: Some(path.clone()),
                info: "The file or path name is too long.  It exceeds the archive header limit."
                    .to_owned(),
                line: 8558,
            },
            Notice::InAnotherBackup { table, dictionary } => Row {
                severity: if *dictionary { 7 } else { 10 },
                code: 7131,
                message: None,
                table: Some(table.clone()),
                info: "Error initializing table for hot backup. See error code for more details."
                    .to_owned(),
                line: if *dictionary { 8048 } else { 8009 },
            },
            Notice::ArchiveBackup { table } => Row {
                severity: 10,
                code: if table.is_some() { 5050 } else { 7040 },
                message: None,
                table: table.clone(),
                info: "Error creating backup archive.".to_owned(),
                line: 9840,
            },
            Notice::DictionaryMetadata { source, code } => Row {
                severity: 10,
                code: *code,
                message: None,
                table: Some(source.clone()),
                info: "Unable to restore the dictionary metadata. Restore operation abandoned. \
                       Verify the target dictionary is not currently opened by another user, or \
                       by the current connection. If you are trying to restore the dictionary \
                       you are currently connected to, use a free connection to call \
                       sp_RestoreDatabase, rather than connecting to the target dictionary and \
                       then calling sp_RestoreDatabase."
                    .to_owned(),
                line: 10655,
            },
            Notice::OpenOrCreate { table, code } => Row {
                severity: 10,
                code: *code,
                message: Some(error_message(*code, None)),
                table: Some(table.clone()),
                info: "Unable to open/create table.".to_owned(),
                line: 11046,
            },
            Notice::OpenSource {
                table,
                code,
                detail,
            } => Row {
                severity: 10,
                code: *code,
                message: Some(error_message(*code, Some(detail))),
                table: Some(table.clone()),
                info: "Unable to open source table.".to_owned(),
                line: 10850,
            },
        }
    }
}

/// The rows a call collects, in the order it processes.
#[derive(Debug, Default)]
pub(crate) struct Rows(Vec<Row>);

impl Rows {
    pub(crate) fn push(&mut self, notice: Notice) {
        self.0.push(notice.row());
    }

    pub(crate) fn into_results(self) -> crate::procedure::Results {
        crate::procedure::Results::new(columns(), self.0.iter().map(Row::values).collect())
    }
}
