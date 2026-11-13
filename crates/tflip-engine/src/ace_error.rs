//! Engine errors as the ACE reports them: the code, and the text
//! `AdsGetLastError` gives, as the original engine words it. tflip-ace
//! records them as a thread's last error; tflip-studio shows them, so its
//! users see the codes and texts they know from ACE.

pub mod codes;
mod texts;

use codes::*;

use crate::Error;

pub use texts::TEXTS;

/// An engine error as the ACE reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AceError {
    pub code: u32,
    pub text: Text,
    /// A failed handle check, which the original engine does before it
    /// clears the last error, so tflip-ace records it without replacing an
    /// error that was pending.
    pub handle_check: bool,
}

/// What `AdsGetLastError` says about an [`AceError`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Text {
    /// The original engine's text of the code.
    Standard,
    /// That text with a detail after it (the field a setter refused, the
    /// file it did not find, ...).
    Detail(String),
    /// A text of its own, whole: an SQL error's, a constraint's.
    Own(String),
}

impl AceError {
    fn new(code: u32, text: Text) -> AceError {
        AceError {
            code,
            text,
            handle_check: false,
        }
    }

    /// What `AdsGetLastError` says about it, as `Error 5004:  <text>`; only
    /// a code the original engine has no text for gets tflip's own words.
    pub fn message(&self) -> String {
        let code = self.code;
        match &self.text {
            Text::Standard => match error_text(code) {
                Some(text) => format!("Error {code}:  {text}"),
                None => format!("Error {code}:  {}", describe(code)),
            },
            Text::Detail(detail) => {
                let text = error_text(code).unwrap_or_default();
                format!("Error {code}:  {text}  {detail}")
            }
            Text::Own(text) => text.clone(),
        }
    }
}

impl Error {
    /// The error as the ACE reports it.
    pub fn to_ace(&self) -> AceError {
        // The transaction errors, with the details the original engine adds.
        let with = |code: u32, detail: &str| {
            let text = match detail.is_empty() {
                true => Text::Own(format!(
                    "Error {code}:  {}",
                    error_text(code).unwrap_or_default()
                )),
                false => Text::Detail(detail.to_owned()),
            };
            AceError::new(code, text)
        };
        let handle = |code: u32, detail: &str| AceError {
            code,
            text: Text::Detail(detail.to_owned()),
            handle_check: true,
        };
        let code = match self {
            Error::Sql(sql) => {
                let mut text = format!("Error 7200:  {sql}");
                // The original engine keeps 562 characters of the error of a
                // trigger that failed, of scripts that nest too deep and of a
                // procedure that fails in a trigger's statement
                // (`fixtures/sql/limits_procs.json`, `trigproc.json`); it
                // does not cut other long ones.
                if text.contains("Trigger execution failed")
                    || text.contains("maximum number of recursion levels")
                {
                    let mut end = text.len().min(562);
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    text.truncate(end);
                }
                return AceError::new(SQL_ERROR, Text::Own(text));
            }
            Error::NotInTransaction => {
                return with(AE_TRANS_OUT_OF_SEQUENCE, "Not in a transaction.");
            }
            Error::AgentFailed => return with(7207, ""),
            Error::IllegalInTransaction(command) => {
                return with(AE_ILLEGAL_COMMAND_DURING_TRANS, command.reason());
            }
            Error::JulianConversion { negative } => {
                return with(AE_INVALID_DATE, crate::julian_conversion_detail(*negative));
            }
            Error::FullText(code) => return with(*code, ""),
            Error::FileRead => return with(7010, ""),
            // A dictionary's constraint or a trigger: the dictionary's message or
            // the engine's own text, as the original says it.
            Error::Constraint { code, text } => {
                return AceError::new(*code, Text::Own(format!("Error {code}:  {text}")));
            }
            Error::InvalidHandle => return handle(AE_INVALID_HANDLE, NOT_A_HANDLE),
            Error::InvalidTable => return handle(AE_INVALID_TABLE_HANDLE, NOT_A_TABLE),
            Error::InvalidConnection => {
                return handle(AE_INVALID_CONNECTION_HANDLE, NOT_A_CONNECTION);
            }
            Error::InvalidStatement => {
                return handle(AE_INVALID_SQLSTATEMENT_HANDLE, NOT_A_STATEMENT);
            }
            Error::InvalidIndex => {
                return AceError {
                    handle_check: true,
                    ..AceError::new(AE_INVALID_INDEX_HANDLE, Text::Standard)
                };
            }
            Error::Ace { code, detail } => {
                let text = match detail.is_empty() {
                    true => Text::Standard,
                    false => Text::Detail(detail.clone()),
                };
                return AceError::new(*code, text);
            }
            Error::Detailed(error, detail) => {
                return AceError::new(error.to_ace().code, Text::Detail(detail.clone()));
            }
            Error::UniqueViolation(order) => {
                let detail = format!("The key value supplied for {order} is not unique.");
                return with(UNIQUE_KEY_VIOLATION, &detail);
            }
            // A referential integrity rule's own message for the refusal.
            Error::NoParentKey(message) => return with(NO_PRIMARY_KEY, message),
            Error::RiRestrict(message) => return with(AE_RI_RESTRICT, message),
            Error::InvalidFieldDefinition(why) => return with(AE_INVALID_FIELDDEF, why),
            Error::NoSuchField(why) => return with(AE_INVALID_EXPRESSION, why),
            Error::RestructureFailed(why) => return with(AE_RESTRUCTURE_FAILED, why),
            // A filter's own calls (AdsClearFilter, AdsGetFilter) add nothing.
            Error::NoFilter => return with(AE_NO_FILTER, "You must call AdsSetAOF first."),
            Error::CyclicRelation(own) => {
                let detail = match own {
                    true => "Index order is on parent table.",
                    false => "",
                };
                return with(AE_CYCLIC_RELATION, detail);
            }
            Error::AutoOpenIndex => AE_AUTOOPEN_INDEX,
            Error::CollationMismatch => AE_COLLATIONS_DO_NOT_MATCH,
            Error::IndexCollationMismatch => AE_INDEX_COLLATION_MISMATCH,
            Error::InvalidIndexOrderName => AE_INVALID_INDEX_ORDER_NAME,
            Error::NoDriveConnection => AE_NO_DRIVE_CONNECTION,
            Error::FileNotFound => AE_FILE_NOT_FOUND,
            Error::RemoteUnavailable => REMOTE_LAYER_UNAVAILABLE,
            Error::CorruptTable => AE_CORRUPT_TABLE,
            Error::SavepointNotFound => AE_SAVEPOINT_NOT_FOUND,
            Error::LockLost => AE_UNLOCK_NOT_LOCKED,
            Error::TransactionLog => FILE_WRITE_PROBLEM,
            Error::RecoveryFailed => FAILED_TRANSACTION_RECOVERY,
            Error::CorruptIndex => AE_CORRUPT_INDEX,
            Error::TableEncrypted => AE_TABLE_ENCRYPTED,
            Error::EncryptionNotEnabled => AE_ENCRYPTION_NOT_ENABLED,
            Error::RecordEncrypted => AE_RECORD_ENCRYPTED,
            Error::InvalidPassword => INVALID_ENCRYPTION_PASSWORD,
            Error::DictionaryEncrypted => ENCRYPTION_REQUIRED,
            Error::DatabaseTable => AE_CANNOT_OPEN_DATABASE_TABLE,
            Error::InvalidFieldNumber => AE_INVALID_FIELDNUMBER,
            Error::InvalidFieldName => AE_INVALID_FIELDNAME,
            Error::InvalidRecordNumber => AE_INVALID_RECORD_NUMBER,
            Error::NoCurrentRecord => AE_NO_CURRENT_RECORD,
            Error::NoPendingUpdate => AE_NO_PENDING_UPDATE,
            Error::InvalidFieldType => AE_INVALID_FIELD_TYPE,
            Error::InvalidDateFormat => AE_INVALID_DATEFORMAT,
            Error::DataTooLong => AE_DATA_TOO_LONG,
            Error::NotFound => AE_NOT_FOUND,
            Error::InvalidExpression => AE_INVALID_EXPRESSION,
            Error::Expression(kind) => kind.code(EXPRESSION_ERRORS),
            Error::Evaluation(runtime) => runtime.code(),
            Error::AofNotLogical => AOF_NOT_LOGICAL,
            Error::InvalidKeyLength => AE_INVALID_KEY_LENGTH,
            Error::InvalidDataType => AE_INVALID_DATA_TYPE,
            Error::InvalidDate => AE_INVALID_DATE,
            Error::ValueOverflow => AE_VALUE_OVERFLOW,
            Error::InvalidData => AE_INVALID_DATA,
            Error::NotImplemented => AE_NOT_IMPLEMENTED,
            Error::NoScope => AE_NO_SCOPE,
            Error::RemoteNotFound => REMOTE_SERVER_NOT_FOUND,
            Error::InvalidFilterOption => AE_INVALID_FILTER_OPTION,
            Error::ConnectionLost => AE_NO_CONNECTION,
            Error::ProtocolMismatch => AE_COMM_MISMATCH,
            Error::FileNotOnServer => AE_FILE_NOT_ON_SERVER,
            Error::InvalidPath => AE_INVALID_PATH,
            Error::FileNotOpen => FILE_NOT_OPEN,
            Error::NotPrepared => AE_NOT_PREPARED,
            Error::CursorNotClosed => AE_CURSOR_NOT_CLOSED,
            Error::StatementLimit => AE_STATEMENT_LIMIT,
            Error::EmptyStatement => AE_INVALID_VALUE,
            Error::InvalidParamNumber => AE_INVALID_SQL_PARAM_NUMBER,
            Error::InvalidParamName => AE_INVALID_SQL_PARAM_NAME,
            Error::NoCursorHandle => AE_INVALID_CURSORHANDLE,
            Error::IsCursorHandle => AE_IS_CURSOR_HANDLE,
            Error::DictionaryUnreadable => DICTIONARY_UNREADABLE,
            Error::LoginFailed => LOGIN_FAILED,
            Error::NoReadRights => NO_READ_RIGHTS,
            Error::PermissionDenied => AE_PERMISSION_DENIED,
            Error::InvalidObjectName => AE_INVALID_OBJECT_NAME,
            Error::NoObjectFound => AE_NO_OBJECT_FOUND,
            Error::InvalidObjectType => AE_INVALID_OBJECT_TYPE,
            Error::PropertyNotSet => AE_PROPERTY_NOT_SET,
            Error::InvalidPropertyId => AE_INVALID_PROPERTY_ID,
            Error::InvalidOption => AE_INVALID_OPTION,
            Error::NotExclusive => AE_TABLE_NOT_EXCLUSIVE,
            Error::FileCreate => FILE_CREATE_ERROR,
            Error::KeyExpression(kind) => kind.code(3000),
            Error::TempTableRefused => TEMP_TABLE_REFUSED,
            Error::LockFailed => AE_LOCK_FAILED,
            Error::RecordNotLocked => AE_RECORD_NOT_LOCKED,
            Error::TableReadOnly => AE_TABLE_READONLY,
            Error::DataTruncated => AE_DATA_TRUNCATED,
            Error::InvalidMonth => AE_INVALID_MONTH,
            Error::InvalidDay(_) => AE_INVALID_DAY,
            Error::InvalidValue => AE_INVALID_VALUE,
            Error::TableNotLocked => AE_TABLE_NOT_LOCKED,
            Error::TableNotShared => AE_TABLE_NOT_SHARED,
            Error::FileInUse => FILE_IN_USE,
            Error::InvalidTableType => AE_INVALID_TABLETYPE,
            Error::WrongTableType(_) => AE_INVALID_TABLETYPE,
            Error::NotVfpNullable => AE_NOT_VFP_NULLABLE_FIELD,
            Error::CorruptMemo => CORRUPT_MEMO,
            Error::KeyOverflow => EXPRESSION_OVERFLOW,
            Error::InvalidMemoBlockSize => AE_INVALID_MEMO_BLOCK_SIZE,
            Error::InvalidRecordLength => AE_INVALID_RECORD_LENGTH,
            Error::NoDictionary => AE_NOT_DICTIONARY,
            Error::NoLocate => AE_LOCATE_NOT_CALLED,
            Error::CustomKey => CUSTOM_KEY_ERROR,
        };
        AceError::new(code, Text::Standard)
    }
}

/// What the original engine adds to the text of a failed handle check: a
/// value that is no handle (`AE_INVALID_HANDLE`)...
pub const NOT_A_HANDLE: &str =
    "The given handle is not recognized as a valid Advantage Client Engine handle.";
/// ... one that is not a table (`AE_INVALID_TABLE_HANDLE`)...
pub const NOT_A_TABLE: &str = "The handle must be to a table.";
/// ... not a connection (`AE_INVALID_CONNECTION_HANDLE`)...
pub const NOT_A_CONNECTION: &str = "The handle must be to an Advantage connection object.";
/// ... and not a statement (`AE_INVALID_SQLSTATEMENT_HANDLE`).
pub const NOT_A_STATEMENT: &str = "The handle must be to a SQL statement.";

/// The original engine's text for the error `code`, if it has one.
pub fn error_text(code: u32) -> Option<&'static str> {
    TEXTS
        .binary_search_by_key(&code, |&(code, _)| code)
        .ok()
        .map(|i| TEXTS[i].1)
}

/// The ACE error `code` as the SQL engine words an error from underneath a
/// statement: the original's text of the code, then `detail`.
pub(crate) fn sql(code: u32, detail: &str) -> Error {
    Error::Sql(tflip_sql::SqlError::ace(
        code,
        error_text(code).unwrap_or_default(),
        detail,
    ))
}

/// An error of a call underneath a statement, as the call itself answers it:
/// [`sql`]'s code and detail, without the SQL engine's wrapping.
pub(crate) fn plain(error: Error) -> Error {
    let Error::Sql(sql) = &error else {
        return error;
    };
    let code = sql.native;
    let described = format!(
        "[ASA] Error {code}:  {}",
        error_text(code).unwrap_or_default()
    );
    match sql.message.strip_prefix(&described) {
        Some(detail) => Error::Ace {
            code,
            detail: detail.strip_prefix("  ").unwrap_or(detail).to_owned(),
        },
        None => error,
    }
}

/// An engine error as the SQL engine words it.
pub(crate) fn sql_error(error: Error) -> Error {
    let error = crate::transaction::sql_error(error);
    if matches!(error, Error::Sql(_)) {
        return error;
    }
    let error = error.to_ace();
    let detail = match &error.text {
        Text::Detail(detail) => detail.as_str(),
        _ => "",
    };
    sql(error.code, detail)
}

/// [`sql`] for the codes the SQL engine words with the text alone and no code.
pub(crate) fn sql_bare(code: u32) -> Error {
    Error::Sql(tflip_sql::SqlError::ace_bare(
        code,
        error_text(code).unwrap_or_default(),
    ))
}

/// A short description of an ACE error code, in tflip's words.
pub fn describe(code: u32) -> &'static str {
    match code {
        AE_SUCCESS => "",
        5002 => "The server speaks another version of the protocol",
        5003 => "Data too long",
        5004 => "File not found",
        5005 => "Buffer too small",
        5006 => "Invalid bookmark",
        5009 => "Invalid date format",
        5014 => "Invalid index handle",
        5016 => "Invalid index order name",
        5018 => "Invalid handle",
        5019 => "Invalid option",
        5020 => "Invalid path",
        5021 => "Invalid pointer",
        5022 => "Invalid record number",
        5023 => "Invalid table handle",
        5024 => "Invalid connection handle",
        5025 => "Invalid table type",
        5030 => "Invalid field number",
        5032 => "Invalid field name",
        5033 => "No such drive",
        5034 => "File not on the server",
        5036 => "The connection to the server is lost",
        5059 => "No file found",
        5066 => "Invalid field type for this operation",
        5037 => "No filter",
        5038 => "No scope",
        5041 => "Not found",
        5068 => "No current record",
        5081 => "Corrupt table",
        5088 => "Corrupt index",
        5098 => "Table is encrypted",
        5100 => "Invalid user name",
        5101 => "Invalid value",
        5106 => "Invalid SQL statement handle",
        5107 => "Invalid SQL cursor handle",
        5108 => "The statement is not prepared",
        5109 => "The cursor of the last execution is still open",
        5110 => "Invalid SQL parameter number",
        5111 => "Invalid SQL parameter name",
        5119 => "Invalid cursor handle: the statement is a SELECT",
        5133 => "Invalid property length",
        5194 => "The table has no memo file",
        5211 => "Characters the code page lacks",
        6080 => "Remote server not available",
        6420 => "Remote server not found",
        7050 => "No such user is connected",
        7051 => "No connection has the file open",
        _ => "Error",
    }
}
