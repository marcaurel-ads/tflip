//! The ACE's error codes: those of `ace.h` the engine's errors map to or
//! tflip-ace returns, and the original engine's that `ace.h` lacks.
//! tflip-ace re-exports them all.

pub const AE_SUCCESS: u32 = 0;
/// The engine opens no converter for the code page (`AdsSetStringFromCodePage`
/// of one it does not know); not an `AE_` code, but what the original returns.
pub const AE_NO_CONVERTER: u32 = 1565;
/// `AdsStmtSetTableCollation` of a `:` locale the engine has no collator for.
pub const AE_COLLATOR_NOT_OPENED: u32 = 1564;
/// `AdsSetCollation` of a name the collation repository lacks.
pub const AE_COLLATION_NAME_NOT_FOUND: u32 = 7147;
pub const AE_DATA_TOO_LONG: u32 = 5003;
pub const AE_FILE_NOT_FOUND: u32 = 5004;
pub const AE_INSUFFICIENT_BUFFER: u32 = 5005;
pub const AE_INVALID_BINARY_OFFSET: u32 = 5082;
/// A copy the server could not make and the client did: left as the last
/// error of a call that succeeded.
pub const AE_INFO_COPY_MADE_BY_CLIENT: u32 = 5169;
pub const AE_INVALID_BOOKMARK: u32 = 5006;
pub const AE_INVALID_DATEFORMAT: u32 = 5009;
pub const AE_INVALID_EXPRESSION: u32 = 5011;
pub const AE_INVALID_FILTER_OPTION: u32 = 5013;
pub const AE_INVALID_INDEX_HANDLE: u32 = 5014;
pub const AE_INVALID_INDEX_TYPE: u32 = 5017;
pub const AE_INDEX_ALREADY_OPEN: u32 = 5065;
pub const AE_AUTOOPEN_INDEX: u32 = 5089;
pub const AE_COLLATIONS_DO_NOT_MATCH: u32 = 5209;
pub const AE_INDEX_COLLATION_MISMATCH: u32 = 5175;
pub const AE_INVALID_INDEX_ORDER: u32 = 5093;
pub const AE_INDEX_EXPR_NOT_FOUND: u32 = 5116;
pub const AE_CORRUPT_INDEX: u32 = 5088;
pub const AE_INVALID_INDEX_ORDER_NAME: u32 = 5016;
pub const AE_INVALID_HANDLE: u32 = 5018;
pub const AE_INVALID_OPTION: u32 = 5019;
pub const AE_INVALID_POINTER: u32 = 5021;
pub const AE_INVALID_RECORD_NUMBER: u32 = 5022;
pub const AE_INVALID_TABLE_HANDLE: u32 = 5023;
pub const AE_INVALID_CONNECTION_HANDLE: u32 = 5024;
pub const AE_INVALID_TABLETYPE: u32 = 5025;
pub const AE_TRANS_OUT_OF_SEQUENCE: u32 = 5047;
pub const AE_ILLEGAL_COMMAND_DURING_TRANS: u32 = 5063;
/// Not in `ace.h`; the original engine's code for an unknown savepoint.
pub const AE_SAVEPOINT_NOT_FOUND: u32 = 7129;
/// Not in `ace.h`; the original engine's code for a commit's release of a
/// record's lock that a rollback to a savepoint had released.
pub const AE_UNLOCK_NOT_LOCKED: u32 = 7138;
pub const AE_CYCLIC_RELATION: u32 = 5077;
/// Not in `ace.h`; the help's "problem with Advantage server file write",
/// for a transaction's log that cannot be written.
pub const FILE_WRITE_PROBLEM: u32 = 7009;
/// Not in `ace.h`; the help's "failed transaction recovery error".
pub const FAILED_TRANSACTION_RECOVERY: u32 = 7036;
pub const AE_INVALID_CHARSETTYPE: u32 = 5027;
pub const AE_INVALID_LOCKTYPE: u32 = 5028;
pub const AE_INVALID_RIGHTSOPTION: u32 = 5029;
pub const AE_INVALID_READONLY_OPTION: u32 = 5114;
pub const AE_INVALID_FIELDNUMBER: u32 = 5030;
pub const AE_COMM_MISMATCH: u32 = 5002;
pub const AE_INVALID_KEY_LENGTH: u32 = 5031;
pub const AE_INVALID_FIELDNAME: u32 = 5032;
pub const AE_NO_FILTER: u32 = 5037;
pub const AE_NO_SCOPE: u32 = 5038;
pub const AE_INVALID_PATH: u32 = 5020;
pub const AE_INVALID_USERNAME: u32 = 5100;
pub const AE_INVALID_VALUE: u32 = 5101;
pub const AE_INVALID_SQLSTATEMENT_HANDLE: u32 = 5106;
pub const AE_INVALID_SQLCURSOR_HANDLE: u32 = 5107;
pub const AE_NOT_PREPARED: u32 = 5108;
pub const AE_CURSOR_NOT_CLOSED: u32 = 5109;
pub const AE_STATEMENT_LIMIT: u32 = 7216;
pub const AE_INVALID_SQL_PARAM_NUMBER: u32 = 5110;
pub const AE_INVALID_SQL_PARAM_NAME: u32 = 5111;
pub const AE_INVALID_COLUMN_NUMBER: u32 = 5112;
pub const AE_INVALID_COLUMN_NAME: u32 = 5113;
pub const AE_IS_CURSOR_HANDLE: u32 = 5115;
pub const AE_INVALID_CONSTRAIN_TYPE: u32 = 5118;
pub const AE_INVALID_CURSORHANDLE: u32 = 5119;
/// An error of the SQL engine; `AdsGetLastError` has its message.
pub const SQL_ERROR: u32 = 7200;
pub const AE_INVALID_PROPERTY_LENGTH: u32 = 5133;
pub const AE_INVALID_PROPERTY_ID: u32 = 5126;
pub const AE_INVALID_MEMO_BLOCK_SIZE: u32 = 5058;
pub const AE_INVALID_RECORD_LENGTH: u32 = 5073;
pub const AE_NOT_DICTIONARY: u32 = 5125;
pub const AE_INVALID_LINK_NUMBER: u32 = 5173;
pub const AE_LOCATE_NOT_CALLED: u32 = 5102;
pub const AE_RESTRUCTURE_FAILED: u32 = 5152;
/// "Error with key in custom index" (`AdsAddCustomKey`).
pub const CUSTOM_KEY_ERROR: u32 = 7037;
pub const AE_INVALID_OBJECT_NAME: u32 = 5132;
pub const AE_INVALID_OBJECT_TYPE: u32 = 5136;
pub const AE_NO_OBJECT_FOUND: u32 = 5137;
pub const AE_PROPERTY_NOT_SET: u32 = 5138;
/// The data dictionary cannot be opened (not in `ace.h`).
pub const DICTIONARY_UNREADABLE: u32 = 7077;
/// The user cannot be authenticated (not in `ace.h`).
pub const LOGIN_FAILED: u32 = 7078;
/// Opening a table the connected user has no read right on.
pub const NO_READ_RIGHTS: u32 = 7079;
pub const AE_NO_DRIVE_CONNECTION: u32 = 5033;
pub const AE_FILE_NOT_ON_SERVER: u32 = 5034;
pub const AE_LOCK_FAILED: u32 = 5035;
pub const AE_NO_CONNECTION: u32 = 5036;
pub const AE_NOT_FOUND: u32 = 5041;
pub const AE_NOT_IMPLEMENTED: u32 = 5042;
pub const AE_BINARY_FILE_ERROR: u32 = 5083;
pub const AE_NO_FILE_FOUND: u32 = 5059;
pub const AE_RECORD_NOT_LOCKED: u32 = 5062;
pub const AE_TABLE_NOT_SHARED: u32 = 5064;
pub const AE_INVALID_FIELD_TYPE: u32 = 5066;
pub const AE_NO_CURRENT_RECORD: u32 = 5068;
pub const AE_NO_PENDING_UPDATE: u32 = 5085;
pub const AE_INVALID_DATA_TYPE: u32 = 5070;
pub const AE_DATA_TRUNCATED: u32 = 5071;
pub const AE_TABLE_READONLY: u32 = 5072;
/// The operation needs a right the connected user does not have.
pub const AE_PERMISSION_DENIED: u32 = 5054;
/// A foreign key has no matching primary key (a referential integrity rule
/// refuses the insert or update through the table API).
pub const NO_PRIMARY_KEY: u32 = 7076;
/// A referential integrity rule set to RESTRICT refuses an update or delete.
pub const AE_RI_RESTRICT: u32 = 5141;
pub const AE_INVALID_DAY: u32 = 5079;
pub const AE_INVALID_MONTH: u32 = 5080;
pub const AE_CORRUPT_TABLE: u32 = 5081;
pub const AE_TABLE_NOT_LOCKED: u32 = 5087;
pub const AE_INVALID_DATE: u32 = 5095;
pub const AE_TABLE_ENCRYPTED: u32 = 5098;
pub const AE_ENCRYPTION_NOT_ENABLED: u32 = 5096;
pub const AE_RECORD_ENCRYPTED: u32 = 5104;
/// A data dictionary's table opened as a free table.
pub const AE_CANNOT_OPEN_DATABASE_TABLE: u32 = 5159;
/// A password that is not the encrypted table's ("Invalid encryption
/// password"; not in `ace.h`).
pub const INVALID_ENCRYPTION_PASSWORD: u32 = 7060;
/// What needs a password was asked for without one: a data dictionary
/// encrypted with AES opened without its `DDPassword` ("not allowed unless
/// encryption was enabled"; not in `ace.h`).
pub const ENCRYPTION_REQUIRED: u32 = 7068;
pub const AE_NO_MEMO_FILE: u32 = 5194;
pub const AE_VALUE_OVERFLOW: u32 = 5179;
pub const AE_NOT_VFP_NULLABLE_FIELD: u32 = 5205;
/// A memo value is corrupt ("Corrupt memo field"; not in `ace.h`).
pub const CORRUPT_MEMO: u32 = 7019;
/// What the original engine refuses a record whose NTX key does not fit
/// with ("evaluator stack overflow"; not in `ace.h`).
pub const EXPRESSION_OVERFLOW: u32 = 4004;
pub const AE_UNICODE_CONVERSION: u32 = 5211;
pub const AE_INVALID_DATA: u32 = 5228;

/// Not in `ace.h`: the code the original engine returns when `AdsOpenTable`
/// is asked for a temporary table (`ADS_TEMP_TABLE`) on a free connection.
pub const TEMP_TABLE_REFUSED: u32 = 7112;

/// Not in `ace.h`: the code the original engine's management functions
/// return for a user (or NetWare connection number) that is not connected.
pub const USER_NOT_FOUND: u32 = 7050;

/// Not in `ace.h`: the code the original engine's `AdsMgGetUserNames`
/// returns for a file no connection has open.
pub const FILE_NOT_OPEN: u32 = 7051;

/// Not in `ace.h`: the code the original engine's `AdsOpenTable101` returns
/// for a table that does not exist (the other open calls return
/// `AE_FILE_NOT_FOUND`).
pub const TABLE_NOT_FOUND: u32 = 7041;
/// A file another handle has open exclusively, or open when it is to be
/// opened exclusively: "The specified table, memo file, or index file was
/// unable to be opened."
pub const FILE_IN_USE: u32 = 7008;
/// A record whose key a unique index order has already: "The record update
/// failed. The key value produced from this record was not unique, ..."
pub const UNIQUE_KEY_VIOLATION: u32 = 7057;

/// Not in `ace.h`: the code the original engine returns when it cannot load
/// its remote communication layer, so it cannot reach a remote server.
pub const REMOTE_LAYER_UNAVAILABLE: u32 = 6080;

/// Not in `ace.h`: the code the original engine returns when connecting by
/// path (`AdsConnect`, `AdsConnect26`, `AdsConnect60`) may only use a remote
/// server and finds none.
pub const REMOTE_SERVER_NOT_FOUND: u32 = 6420;

pub const AE_INVALID_FIELDDEF: u32 = 5012;
pub const AE_TABLE_NOT_EXCLUSIVE: u32 = 5067;

/// Not in `ace.h`: a table file that cannot be created (it exists).
pub const FILE_CREATE_ERROR: u32 = 7040;

/// The first of the codes of expressions that do not compile when they
/// are evaluated (`AdsEval*Expr`, `AdsLocate`): 3501 incomplete, 3511
/// unknown identifier, ... (see `ErrorKind::code` in `tflip-expr`).
pub const EXPRESSION_ERRORS: u32 = 3500;
/// The same as record filters (`AdsSetFilter`, `AdsSetAOF`): 3101, ...
pub const FILTER_ERRORS: u32 = 3100;
/// The same for `AdsIsExprValid`: 3401, ...
pub const VALIDITY_ERRORS: u32 = 3400;
/// "AOF error: Invalid expression data type": an AOF that is not logical.
pub const AOF_NOT_LOGICAL: u32 = 6209;
