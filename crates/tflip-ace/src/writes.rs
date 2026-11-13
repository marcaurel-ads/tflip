//! The write scenarios of `fixtures/write/`, replayed through tflip's
//! exports: the Rust twin of `tools/ace/write/write.c`, which recorded what
//! the original engine does with the same scripts. Each scenario's
//! `ops.txt` runs on a copy of its `before/` folder; every command must
//! return what the original returned (and give the same outputs), and the
//! folder must end up as the original's `after/`, byte for byte but for
//! the bytes that hold the time of a write (ADT ModTime fields, a DBF
//! header's date) and those the original leaves uninitialised (the
//! scenario's `volatile.json`). Each scenario runs on a copy in the process
//! and through a server.

use std::ffi::CString;
use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};
use tflip_storage::field::FieldType;
use tflip_storage::table::{Format, Structure};

use crate::connection::*;
use crate::field::*;
use crate::fixtures::{self, remote_twins};

use crate::index::*;
use crate::record::*;
use crate::setters::*;
use crate::statement::*;
use crate::table::*;
use crate::*;

fn scenario_dir(name: &str) -> PathBuf {
    fixtures::dir("").join("../write").join(name)
}

/// The next space-separated word of `rest`, which moves past it.
fn word<'a>(rest: &mut &'a str) -> &'a str {
    let trimmed = rest.trim_start_matches(' ');
    let end = trimmed.find(' ').unwrap_or(trimmed.len());
    let (word, after) = trimmed.split_at(end);
    *rest = after.strip_prefix(' ').unwrap_or(after);
    word
}

/// The bytes of a text value: Windows-1252, with `\r`, `\n`, `\t`, `\\`,
/// `\xHH` and `\@NNNNNc` (NNNNN times c) escapes, `""` for empty.
fn unescape(text: &str) -> Vec<u8> {
    if text == "\"\"" {
        return Vec::new();
    }
    let bytes = tflip_storage::cp1252::encode(text);
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 == bytes.len() {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        i += 1;
        match bytes[i] {
            // `\@NNNNNc`: NNNNN times c.
            b'@' if i + 6 < bytes.len() => {
                let count: usize = std::str::from_utf8(&bytes[i + 1..i + 6])
                    .unwrap()
                    .parse()
                    .unwrap();
                out.extend(std::iter::repeat_n(bytes[i + 6], count));
                i += 7;
            }
            b'x' if i + 2 < bytes.len() => {
                let digits = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
                out.push(u8::from_str_radix(digits, 16).unwrap());
                i += 3;
            }
            c => {
                out.push(match c {
                    b'r' => b'\r',
                    b'n' => b'\n',
                    b't' => b'\t',
                    c => c,
                });
                i += 1;
            }
        }
    }
    out
}

/// The bytes of hex text, a pair of characters each, as `write.c` reads
/// them (`strtoul`): the hex digits a pair starts with, 0 if none (so `""`
/// is one zero byte).
fn from_hex(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    (0..bytes.len() / 2)
        .map(|i| {
            let pair = &bytes[2 * i..2 * i + 2];
            let digits = pair.iter().take_while(|b| b.is_ascii_hexdigit()).count();
            u8::from_str_radix(std::str::from_utf8(&pair[..digits]).unwrap(), 16).unwrap_or(0)
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A field argument: `#n` for field number `n`, else a name.
enum FieldArg {
    Number(usize),
    Name(CString),
}

impl FieldArg {
    fn new(text: &str) -> FieldArg {
        match text.strip_prefix('#') {
            Some(number) => FieldArg::Number(number.parse().unwrap()),
            None => FieldArg::Name(CString::new(text).unwrap()),
        }
    }

    fn ptr(&self) -> *mut Unsigned8 {
        match self {
            FieldArg::Number(n) => *n as *mut Unsigned8,
            FieldArg::Name(name) => name.as_ptr() as *mut Unsigned8,
        }
    }
}

/// Runs one setter, as `write.c`'s `set` does.
fn set(table: AdsHandle, field: &FieldArg, kind: &str, value: &str) -> Unsigned32 {
    let f = field.ptr();
    let int = || value.trim().parse::<i64>().unwrap();
    unsafe {
        match kind {
            "null" => AdsSetNull(table, f),
            "empty" => AdsSetEmpty(table, f),
            "binary" | "image" => {
                let mut bytes = from_hex(value);
                let kind = if kind == "binary" {
                    ADS_BINARY
                } else {
                    ADS_IMAGE
                };
                let n = bytes.len() as u32;
                AdsSetBinary(table, f, kind, n, 0, bytes.as_mut_ptr(), n)
            }
            "tsraw" => {
                let mut bytes = from_hex(value);
                crate::AdsSetTimeStampRaw(table, f, bytes.as_mut_ptr(), bytes.len() as u32)
            }
            "double" => AdsSetDouble(table, f, value.trim().parse().unwrap()),
            "long" => AdsSetLong(table, f, int() as i32),
            "julian" => AdsSetJulian(table, f, int() as i32),
            "ms" => AdsSetMilliseconds(table, f, int() as i32),
            "longlong" => AdsSetLongLong(table, f, int()),
            "money" => AdsSetMoney(table, f, int()),
            "short" => AdsSetShort(table, f, int() as i16),
            "logical" => AdsSetLogical(table, f, int() as u16),
            "stringu" | "fieldu" => {
                let mut units: Vec<u16> = from_hex(value)
                    .chunks(2)
                    .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
                    .collect();
                let n = units.len() as u32;
                match kind {
                    "stringu" => AdsSetStringW(table, f, units.as_mut_ptr(), n),
                    _ => AdsSetFieldW(table, f, units.as_mut_ptr(), n),
                }
            }
            _ => {
                let mut bytes = unescape(value);
                let n = bytes.len();
                match kind {
                    "date" => AdsSetDate(table, f, bytes.as_mut_ptr(), n as u16),
                    "time" => AdsSetTime(table, f, bytes.as_mut_ptr(), n as u16),
                    "timestamp" => AdsSetTimeStamp(table, f, bytes.as_mut_ptr(), n as u32),
                    "string" => AdsSetString(table, f, bytes.as_mut_ptr(), n as u32),
                    "field" => AdsSetField(table, f, bytes.as_mut_ptr(), n as u32),
                    "stringw" | "fieldw" => {
                        let text = tflip_storage::cp1252::decode(&bytes);
                        let mut units: Vec<u16> = text.encode_utf16().collect();
                        let n = units.len() as u32;
                        match kind {
                            "stringw" => AdsSetStringW(table, f, units.as_mut_ptr(), n),
                            _ => AdsSetFieldW(table, f, units.as_mut_ptr(), n),
                        }
                    }
                    _ => panic!("unknown setter kind {kind}"),
                }
            }
        }
    }
}

/// Runs a scenario's `ops.txt` in the folder `source` (a path, or a server
/// path): what each command returned, as `write.c` records it (without the
/// error texts).
/// The files of a folder at a `snapshot` of a script, by name.
type Snapshot = (String, Vec<(std::ffi::OsString, Vec<u8>)>);

pub(crate) fn replay(source: &str, work: &Path, script: &str) -> (Vec<Json>, Vec<Snapshot>) {
    let mut results = Vec::new();
    let mut snapshots = Vec::new();
    let mut connection = fixtures::connect_to(source);
    // A second connection (`open2`), as another user.
    let mut connection2 = 0;
    // tflip finds names in the connection's folder (`write.c` gives the
    // original engine full paths, which it needs).
    let in_folder = |name: &str| CString::new(name).unwrap();
    let mut statement = 0;
    let mut tables: Vec<AdsHandle> = Vec::new();
    // The index order each table moves in (`order`), 0 for record order.
    let mut orders: Vec<AdsHandle> = Vec::new();
    let mut current = 0usize;
    // `use conn`: the calls on the current table go to the connection.
    let mut on_connection = false;
    let (mut table_type, mut char_type) = (ADS_ADT, ADS_ANSI);
    // `cursor`: the statements of the cursors, by table number, and the
    // settings the next one gets (`constrain`, `readall`).
    let mut cursor_statements: Vec<AdsHandle> = Vec::new();
    let (mut constrain, mut read_columns, mut cursor_read_only) = (None, None, None);
    for line in script.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut rest = line;
        let cmd = word(&mut rest);
        let table = match (on_connection, current) {
            (true, _) => connection,
            (false, 0) => 0,
            (false, n) => tables[n - 1],
        };
        let mut result = json!({ "cmd": line });
        let simple: Option<extern "system" fn(AdsHandle) -> Unsigned32> = match cmd {
            "append" => Some(AdsAppendRecord),
            "write" => Some(AdsWriteRecord),
            "delete" => Some(AdsDeleteRecord),
            "recall" => Some(AdsRecallRecord),
            "cancel" => Some(AdsCancelUpdate),
            "refresh" => Some(AdsRefreshRecord),
            "top" => Some(AdsGotoTop),
            "bottom" => Some(AdsGotoBottom),
            "locktable" => Some(AdsLockTable),
            "unlocktable" => Some(AdsUnlockTable),
            "close" => Some(AdsCloseTable),
            "clearaof" => Some(crate::AdsClearAOF),
            "refreshaof" => Some(crate::AdsRefreshAOF),
            "clearfilter" => Some(crate::AdsClearFilter),
            "reindex" => Some(crate::AdsReindex),
            "pack" => Some(crate::AdsPackTable),
            "zap" => Some(crate::AdsZapTable),
            "recallall" => Some(crate::AdsRecallAllRecords),
            "deletetable" => Some(crate::AdsDeleteTable),
            "flush" => Some(AdsFlushFileBuffers),
            "activateaof" => Some(crate::AdsActivateAOF),
            "deactivateaof" => Some(crate::AdsDeactivateAOF),
            "clearrelation" => Some(crate::AdsClearRelation),
            "encrypttable" => Some(crate::AdsEncryptTable),
            "decrypttable" => Some(crate::AdsDecryptTable),
            "encryptrecord" => Some(crate::AdsEncryptRecord),
            "decryptrecord" => Some(crate::AdsDecryptRecord),
            "disableenc" => Some(crate::AdsDisableEncryption),
            _ => None,
        };
        let rc = match (simple, cmd) {
            (Some(f), _) => f(table),
            (None, "sql") => unsafe {
                if statement == 0 {
                    AdsCreateSQLStatement(connection, &mut statement);
                }
                let mut sql = unescape(rest);
                sql.push(0);
                AdsExecuteSQLDirect(statement, sql.as_mut_ptr(), std::ptr::null_mut())
            },
            (None, "tabletype") => unsafe {
                table_type = match rest.trim() {
                    "ntx" => ADS_NTX,
                    "cdx" => ADS_CDX,
                    "vfp" => ADS_VFP,
                    "adt" => ADS_ADT,
                    "database" => crate::ADS_DATABASE_TABLE,
                    other => other
                        .parse()
                        .unwrap_or_else(|_| panic!("unknown table type {other}")),
                };
                if statement == 0 {
                    AdsCreateSQLStatement(connection, &mut statement);
                }
                AdsStmtSetTableType(statement, table_type)
            },
            (None, "chartype") => {
                char_type = if rest.trim() == "oem" {
                    ADS_OEM
                } else {
                    ADS_ANSI
                };
                if statement == 0 {
                    unsafe { AdsCreateSQLStatement(connection, &mut statement) };
                }
                AdsStmtSetTableCharType(statement, char_type)
            }
            (None, "otop" | "obottom" | "oskip" | "seek") => {
                let order = orders[current - 1];
                let moving = if order != 0 { order } else { table };
                match cmd {
                    "otop" => AdsGotoTop(moving),
                    "obottom" => AdsGotoBottom(moving),
                    "oskip" => AdsSkip(moving, rest.trim().parse().unwrap()),
                    _ => {
                        let seek_type: u16 = word(&mut rest).parse().unwrap();
                        let mut key = unescape(rest);
                        let len = key.len() as u16;
                        let mut found = 77;
                        let rc = unsafe {
                            crate::AdsSeek(moving, key.as_mut_ptr(), len, 2, seek_type, &mut found)
                        };
                        result["found"] = json!(found);
                        rc
                    }
                }
            }
            (None, "pos") => {
                let (mut recno, mut bof, mut eof, mut found) = (0, 77, 77, 77);
                let rcs = unsafe {
                    [
                        AdsGetRecordNum(table, ADS_IGNOREFILTERS, &mut recno),
                        crate::AdsAtBOF(table, &mut bof),
                        AdsAtEOF(table, &mut eof),
                        crate::AdsIsFound(table, &mut found),
                    ]
                };
                result["pos"] = json!([recno, bof, eof, found]);
                result["rcs"] = json!(rcs);
                AE_SUCCESS
            }
            (None, "scope" | "getscope" | "clearscope") => {
                let order = orders[current - 1];
                let option: u16 = word(&mut rest).parse().unwrap();
                match cmd {
                    "scope" => {
                        let mut key = unescape(rest);
                        let len = key.len() as u16;
                        unsafe { crate::AdsSetScope(order, option, key.as_mut_ptr(), len, 2) }
                    }
                    "clearscope" => crate::AdsClearScope(order, option),
                    _ => {
                        let mut buf = [0u8; 300];
                        let mut len = buf.len() as u16;
                        let rc = unsafe {
                            crate::AdsGetScope(order, option, buf.as_mut_ptr(), &mut len)
                        };
                        let shown = if rc == AE_SUCCESS {
                            usize::from(len)
                        } else {
                            0
                        };
                        result["scope"] = json!(hex(&buf[..shown]));
                        rc
                    }
                }
            }
            (None, "relation" | "scopedrelation") => {
                let which = word(&mut rest);
                let child = match which.strip_prefix('t') {
                    Some(n) => tables[n.parse::<usize>().unwrap() - 1],
                    None => match which.parse::<usize>().unwrap() {
                        0 => 0,
                        n if orders[n - 1] != 0 => orders[n - 1],
                        n => tables[n - 1],
                    },
                };
                let mut expr = unescape(rest);
                expr.push(0);
                let expr = if rest == "-" {
                    std::ptr::null_mut()
                } else {
                    expr.as_mut_ptr()
                };
                match cmd {
                    "relation" => unsafe { crate::AdsSetRelation(table, child, expr) },
                    _ => unsafe { crate::AdsSetScopedRelation(table, child, expr) },
                }
            }
            (None, "showdeleted") => AdsShowDeleted(rest.trim().parse().unwrap()),
            (None, "exact") => crate::settings::AdsSetExact(rest.trim().parse().unwrap()),
            (None, "collation") => unsafe {
                let mut name = rest.as_bytes().to_vec();
                name.push(0);
                crate::settings::AdsSetCollation(connection, name.as_mut_ptr())
            },
            (None, "open") => {
                let name = word(&mut rest);
                let options = match word(&mut rest) {
                    "" | "exclusive" => ADS_EXCLUSIVE,
                    "shared" => ADS_SHARED,
                    "readonly" => ADS_READONLY | ADS_SHARED,
                    n => n.parse().unwrap(),
                };
                let file = in_folder(name.strip_prefix(':').unwrap_or(name));
                let mut handle = 0;
                let rc = unsafe {
                    AdsOpenTable(
                        connection,
                        file.as_ptr() as *mut Unsigned8,
                        std::ptr::null_mut(),
                        table_type,
                        char_type,
                        ADS_PROPRIETARY_LOCKING,
                        1,
                        options,
                        &mut handle,
                    )
                };
                if rc == AE_SUCCESS {
                    tables.push(handle);
                    orders.push(0);
                    current = tables.len();
                    result["table"] = json!(current);
                }
                rc
            }
            (None, "open2") => {
                let name = word(&mut rest);
                let options = match word(&mut rest) {
                    "" | "exclusive" => ADS_EXCLUSIVE,
                    "shared" => ADS_SHARED,
                    "readonly" => ADS_READONLY | ADS_SHARED,
                    n => n.parse().unwrap(),
                };
                if connection2 == 0 {
                    connection2 = fixtures::connect_to(source);
                }
                let file = in_folder(name.strip_prefix(':').unwrap_or(name));
                let mut handle = 0;
                let rc = unsafe {
                    AdsOpenTable(
                        connection2,
                        file.as_ptr() as *mut Unsigned8,
                        std::ptr::null_mut(),
                        table_type,
                        char_type,
                        ADS_PROPRIETARY_LOCKING,
                        1,
                        options,
                        &mut handle,
                    )
                };
                if rc == AE_SUCCESS {
                    tables.push(handle);
                    orders.push(0);
                    current = tables.len();
                    result["table"] = json!(current);
                }
                rc
            }
            // Connects to the script's source again as a user, as the
            // connection of the server the script runs on does.
            (None, "reconnect") => {
                let user = word(&mut rest);
                let password = word(&mut rest).replace('-', "");
                let (rc, fresh) = fixtures::connect_to_as(source, user, &password);
                if rc == AE_SUCCESS {
                    connection = fresh;
                    statement = 0;
                }
                rc
            }
            (None, "ddcreate" | "ddconnect") => unsafe {
                let mut rest = rest;
                let mut path = format!("{}/{}\0", source, word(&mut rest)).into_bytes();
                let user = CString::new(word(&mut rest)).unwrap();
                let password = CString::new(word(&mut rest).replace('-', "")).unwrap();
                let named = !user.as_bytes().is_empty();
                let mut fresh = 0;
                let rc = match cmd {
                    "ddcreate" => {
                        crate::AdsDDCreate(path.as_mut_ptr(), 0, std::ptr::null_mut(), &mut fresh)
                    }
                    _ => AdsConnect60(
                        path.as_mut_ptr(),
                        ADS_LOCAL_SERVER,
                        if named {
                            user.as_ptr().cast_mut().cast()
                        } else {
                            std::ptr::null_mut()
                        },
                        if named {
                            password.as_ptr().cast_mut().cast()
                        } else {
                            std::ptr::null_mut()
                        },
                        0,
                        &mut fresh,
                    ),
                };
                if rc == AE_SUCCESS {
                    connection = fresh;
                    statement = 0;
                }
                rc
            },
            (None, "constrain" | "readall" | "cursorreadonly") => {
                let value = Some(rest.trim().parse().unwrap());
                match cmd {
                    "constrain" => constrain = value,
                    "readall" => read_columns = value,
                    _ => cursor_read_only = value,
                }
                AE_SUCCESS
            }
            (None, "cursor" | "cursor2") => unsafe {
                let mut cursor_statement = 0;
                let mut cursor = 0;
                AdsCreateSQLStatement(
                    if cmd == "cursor2" {
                        connection2
                    } else {
                        connection
                    },
                    &mut cursor_statement,
                );
                AdsStmtSetTableType(cursor_statement, table_type);
                AdsStmtSetTableCharType(cursor_statement, char_type);
                let mut rc = AE_SUCCESS;
                if let (AE_SUCCESS, Some(value)) = (rc, constrain) {
                    rc = AdsStmtConstrainUpdates(cursor_statement, value);
                }
                if let (AE_SUCCESS, Some(value)) = (rc, cursor_read_only) {
                    rc = AdsStmtSetTableReadOnly(cursor_statement, value);
                }
                if let (AE_SUCCESS, Some(value)) = (rc, read_columns) {
                    rc = AdsStmtReadAllColumns(cursor_statement, value);
                }
                let mut sql = unescape(rest);
                sql.push(0);
                if rc == AE_SUCCESS {
                    rc = AdsExecuteSQLDirect(cursor_statement, sql.as_mut_ptr(), &mut cursor);
                }
                if rc == AE_SUCCESS && cursor != 0 {
                    cursor_statements.resize(tables.len(), 0);
                    cursor_statements.push(cursor_statement);
                    tables.push(cursor);
                    orders.push(0);
                    current = tables.len();
                    result["table"] = json!(current);
                }
                rc
            },
            (None, "stmtset") => {
                let what = word(&mut rest);
                let value = rest.trim().parse().unwrap();
                let cursor_statement = cursor_statements[current - 1];
                match what {
                    "constrain" => AdsStmtConstrainUpdates(cursor_statement, value),
                    "readall" => AdsStmtReadAllColumns(cursor_statement, value),
                    _ => AdsStmtSetTableReadOnly(cursor_statement, value),
                }
            }
            (None, "closecursor") => AdsCloseSQLStatement(cursor_statements[current - 1]),
            (None, "begin") => crate::AdsBeginTransaction(connection),
            (None, "commit") => crate::AdsCommitTransaction(connection),
            (None, "rollback") => crate::AdsRollbackTransaction(connection),
            (None, "savepoint" | "rollbackto") => {
                let name = in_folder(rest.trim());
                let name = name.as_ptr() as *mut Unsigned8;
                match cmd {
                    "savepoint" => unsafe { crate::AdsCreateSavepoint(connection, name, 0) },
                    _ => unsafe { crate::AdsRollbackTransaction80(connection, name, 0) },
                }
            }
            (None, "txcount") => {
                let mut count = 0;
                let rc = unsafe { crate::AdsGetTransactionCount(connection, &mut count) };
                result["count"] = json!(count);
                rc
            }
            (None, "snapshot") => {
                let mut files = Vec::new();
                for entry in std::fs::read_dir(work).unwrap() {
                    let entry = entry.unwrap();
                    files.push((entry.file_name(), std::fs::read(entry.path()).unwrap()));
                }
                snapshots.push((rest.trim().to_owned(), files));
                AE_SUCCESS
            }
            // The files of another folder, such as a backup's image.
            (None, "snapdir") => {
                let name = word(&mut rest).to_owned();
                let mut files = Vec::new();
                for entry in std::fs::read_dir(rest.trim()).unwrap() {
                    let entry = entry.unwrap();
                    if entry.file_type().unwrap().is_file() {
                        files.push((entry.file_name(), std::fs::read(entry.path()).unwrap()));
                    }
                }
                snapshots.push((name, files));
                AE_SUCCESS
            }
            (None, "use") => {
                on_connection = rest.trim() == "conn";
                if !on_connection {
                    current = rest.trim().parse().unwrap();
                }
                AE_SUCCESS
            }
            (None, "openindex") => {
                let file = in_folder(rest);
                let mut orders = [0; 256];
                let mut n = 256;
                unsafe {
                    AdsOpenIndex(
                        table,
                        file.as_ptr() as *mut Unsigned8,
                        orders.as_mut_ptr(),
                        &mut n,
                    )
                }
            }
            (None, "writeall") => AdsWriteAllRecords(),
            (None, "goto") => AdsGotoRecord(table, rest.trim().parse().unwrap()),
            (None, "lock") => AdsLockRecord(table, rest.trim().parse().unwrap()),
            (None, "unlock") => AdsUnlockRecord(table, rest.trim().parse().unwrap()),
            (None, "skip") => AdsSkip(table, rest.trim().parse().unwrap()),
            (None, "islocked") => {
                let mut locked = 0;
                let rc =
                    unsafe { AdsIsRecordLocked(table, rest.trim().parse().unwrap(), &mut locked) };
                result["locked"] = json!(locked);
                rc
            }
            (None, "tablelocked") => {
                let mut locked = 0;
                let rc = unsafe { AdsIsTableLocked(table, &mut locked) };
                result["locked"] = json!(locked);
                rc
            }
            (None, "locks") => {
                let mut locks = [0u32; 256];
                let mut n = 256;
                let rc = unsafe { AdsGetAllLocks(table, locks.as_mut_ptr(), &mut n) };
                let n = if rc == AE_SUCCESS {
                    usize::from(n).min(256)
                } else {
                    0
                };
                result["locks"] = json!(locks[..n]);
                rc
            }
            (None, "set") => {
                let field = FieldArg::new(word(&mut rest));
                let kind = word(&mut rest);
                set(table, &field, kind, rest)
            }
            (None, "get") => {
                let field = FieldArg::new(word(&mut rest));
                let mut null = 0;
                let null_rc = unsafe { AdsIsNull(table, field.ptr(), &mut null) };
                let mut buf = vec![0u8; 70000];
                let mut len = buf.len() as u32;
                let rc = unsafe { AdsGetField(table, field.ptr(), buf.as_mut_ptr(), &mut len, 0) };
                result["null"] = json!(if null_rc == AE_SUCCESS { null } else { 99 });
                let shown = if (len as usize) < buf.len() {
                    len as usize
                } else {
                    0
                };
                result["value"] = json!(hex(&buf[..shown]));
                rc
            }
            (None, "record") => {
                let mut buf = vec![0u8; 70000];
                let mut len = buf.len() as u32;
                let rc = unsafe { AdsGetRecord(table, buf.as_mut_ptr(), &mut len) };
                let shown = if rc == AE_SUCCESS { len as usize } else { 0 };
                result["record"] = json!(hex(&buf[..shown]));
                rc
            }
            (None, "raw") => {
                let field = FieldArg::new(word(&mut rest));
                let mut buf = vec![0u8; 70000];
                let mut len = buf.len() as u32;
                let (mut offset, mut number, mut fields) = (0, 0, 0);
                let mut rc = unsafe { AdsGetRecord(table, buf.as_mut_ptr(), &mut len) };
                if rc == AE_SUCCESS {
                    rc = unsafe { AdsGetFieldOffset(table, field.ptr(), &mut offset) };
                }
                if rc == AE_SUCCESS {
                    rc = unsafe { AdsGetFieldNum(table, field.ptr(), &mut number) };
                }
                unsafe { AdsGetNumFields(table, &mut fields) };
                let mut next = len;
                if rc == AE_SUCCESS && number < fields {
                    let after = usize::from(number + 1) as *mut Unsigned8;
                    unsafe { AdsGetFieldOffset(table, after, &mut next) };
                }
                let shown = match rc == AE_SUCCESS && next >= offset {
                    true => &buf[offset as usize..next as usize],
                    false => &[][..],
                };
                result["raw"] = json!(hex(shown));
                rc
            }
            (None, "state") => {
                let (mut recno, mut records, mut autoinc, mut deleted) = (0, 0, 0, 0);
                let r1 = unsafe { AdsGetRecordNum(table, ADS_IGNOREFILTERS, &mut recno) };
                let r2 = unsafe { AdsGetRecordCount(table, ADS_IGNOREFILTERS, &mut records) };
                let r3 = unsafe { AdsIsRecordDeleted(table, &mut deleted) };
                let r4 = unsafe { AdsGetLastAutoinc(table, &mut autoinc) };
                result["recno"] = json!([r1, recno]);
                result["count"] = json!([r2, records]);
                result["deleted"] = json!([r3, deleted]);
                result["autoinc"] = json!([r4, autoinc]);
                AE_SUCCESS
            }
            (None, "enableenc") => {
                let mut password = unescape(rest);
                password.push(0);
                unsafe { crate::AdsEnableEncryption(table, password.as_mut_ptr()) }
            }
            (None, "encstate") => {
                let (mut enabled, mut record, mut encrypted) = (77, 77, 77);
                let rcs = unsafe {
                    [
                        crate::AdsIsEncryptionEnabled(table, &mut enabled),
                        crate::AdsIsRecordEncrypted(table, &mut record),
                        crate::AdsIsTableEncrypted(table, &mut encrypted),
                    ]
                };
                result["enabled"] = json!([rcs[0], enabled]);
                result["record"] = json!([rcs[1], record]);
                result["table"] = json!([rcs[2], encrypted]);
                AE_SUCCESS
            }
            (None, "info") => {
                let mut length = 77;
                let (mut fields, mut block, mut encrypted, mut kind) = (77, 77, 77, 77);
                let rcs = unsafe {
                    [
                        crate::AdsGetRecordLength(table, &mut length),
                        AdsGetNumFields(table, &mut fields),
                        crate::AdsGetMemoBlockSize(table, &mut block),
                        crate::AdsIsTableEncrypted(table, &mut encrypted),
                        crate::AdsGetTableType(table, &mut kind),
                    ]
                };
                result["length"] = json!([rcs[0], length]);
                result["fields"] = json!([rcs[1], fields]);
                result["block"] = json!([rcs[2], block]);
                result["encrypted"] = json!([rcs[3], encrypted]);
                result["type"] = json!([rcs[4], kind]);
                AE_SUCCESS
            }
            (None, "createtable") => {
                let name = in_folder(word(&mut rest));
                let memo: u16 = word(&mut rest).parse().unwrap();
                let mut fields = unescape(rest);
                fields.push(0);
                let mut handle = 0;
                let rc = unsafe {
                    crate::AdsCreateTable71(
                        connection,
                        name.as_ptr() as *mut Unsigned8,
                        std::ptr::null_mut(),
                        table_type,
                        char_type,
                        ADS_PROPRIETARY_LOCKING,
                        1,
                        memo,
                        fields.as_mut_ptr(),
                        0,
                        &mut handle,
                    )
                };
                if rc == AE_SUCCESS {
                    tables.push(handle);
                    orders.push(0);
                    current = tables.len();
                    result["table"] = json!(current);
                }
                rc
            }
            (None, "createindex") => {
                let file = word(&mut rest);
                let tag = CString::new(word(&mut rest)).unwrap();
                let options: u32 = word(&mut rest).parse().unwrap();
                let page: u32 = word(&mut rest).parse().unwrap();
                let (expression, condition) = match rest.split_once(" FOR ") {
                    Some((e, c)) => (e, c),
                    None => (rest, ""),
                };
                let file = (file != "-").then(|| in_folder(file));
                let expression = CString::new(unescape(expression)).unwrap();
                let condition = CString::new(unescape(condition)).unwrap();
                let mut order = 0;
                unsafe {
                    crate::AdsCreateIndex61(
                        table,
                        file.as_ref()
                            .map_or(std::ptr::null_mut(), |f| f.as_ptr() as *mut _),
                        tag.as_ptr() as *mut _,
                        expression.as_ptr() as *mut _,
                        condition.as_ptr() as *mut _,
                        c"".as_ptr() as *mut _,
                        options,
                        page,
                        &mut order,
                    )
                }
            }
            (None, "createfts") => {
                let number = |text: &str| match text.strip_prefix("0x") {
                    Some(hex) => u32::from_str_radix(hex, 16).unwrap(),
                    None => text.parse().unwrap(),
                };
                let file = word(&mut rest);
                let file = (file != "-").then(|| in_folder(file));
                let tag = CString::new(word(&mut rest)).unwrap();
                let field = CString::new(word(&mut rest)).unwrap();
                let [page, min, max, options] = [(); 4].map(|()| number(word(&mut rest)));
                let mut lists = Vec::new();
                for _ in 0..4 {
                    let used: u16 = word(&mut rest).parse().unwrap();
                    let text = word(&mut rest);
                    let text = (text != "-").then(|| {
                        let mut bytes = unescape(text);
                        bytes.push(0);
                        bytes
                    });
                    lists.push((used, text));
                }
                let pointer = |text: &Option<Vec<u8>>| {
                    text.as_ref().map_or(std::ptr::null_mut(), |t| {
                        t.as_ptr() as *mut std::ffi::c_void
                    })
                };
                unsafe {
                    crate::AdsCreateFTSIndex(
                        table,
                        file.as_ref()
                            .map_or(std::ptr::null_mut(), |f| f.as_ptr() as *mut _),
                        tag.as_ptr() as *mut _,
                        field.as_ptr() as *mut _,
                        page,
                        min,
                        max,
                        lists[0].0,
                        pointer(&lists[0].1),
                        lists[1].0,
                        pointer(&lists[1].1),
                        lists[2].0,
                        pointer(&lists[2].1),
                        lists[3].0,
                        pointer(&lists[3].1),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        options,
                    )
                }
            }
            (None, "reindexfts") => crate::AdsReindexFTS(table, rest.trim().parse().unwrap()),
            (None, "deleteindex") => {
                let name = CString::new(rest).unwrap();
                let mut order = 0;
                let mut rc =
                    unsafe { AdsGetIndexHandle(table, name.as_ptr() as *mut _, &mut order) };
                if rc == AE_SUCCESS {
                    rc = crate::AdsDeleteIndex(order);
                }
                rc
            }
            (None, "aof") => {
                let options: u16 = word(&mut rest).parse().unwrap();
                let mut text = unescape(rest);
                text.push(0);
                unsafe { crate::AdsSetAOF(table, text.as_mut_ptr(), options) }
            }
            (None, "filter") => {
                let mut text = unescape(rest);
                text.push(0);
                unsafe { crate::AdsSetFilter(table, text.as_mut_ptr()) }
            }
            (None, "aoflevel") => {
                let (mut level, mut buf, mut len) = (0u16, vec![0u8; 4096], 4096u16);
                let rc = unsafe {
                    crate::AdsGetAOFOptLevel(table, &mut level, buf.as_mut_ptr(), &mut len)
                };
                let shown = if rc == AE_SUCCESS {
                    usize::from(len)
                } else {
                    0
                };
                result["level"] = json!(level);
                result["nonopt"] =
                    json!(buf[..shown].iter().map(|&b| b as char).collect::<String>());
                rc
            }
            (None, "evalaof") => {
                let mut level = 0u16;
                let mut text = unescape(rest);
                text.push(0);
                let rc = unsafe { crate::AdsEvalAOF(table, text.as_mut_ptr(), &mut level) };
                result["level"] = json!(level);
                rc
            }
            (None, "inaof") => {
                let mut flag = 0u16;
                let record = rest.trim().parse().unwrap();
                let rc = unsafe { crate::AdsIsRecordInAOF(table, record, &mut flag) };
                result["flag"] = json!(flag);
                rc
            }
            (None, "custom") => {
                let option: u16 = word(&mut rest).parse().unwrap();
                let mut records: Vec<u32> = rest
                    .split_whitespace()
                    .map(|n| n.parse().unwrap())
                    .collect();
                let n = records.len() as u32;
                unsafe { crate::AdsCustomizeAOF(table, n, records.as_mut_ptr(), option) }
            }
            (None, "order") => {
                let mut order = 0;
                let rc = match rest {
                    "-" => AE_SUCCESS,
                    name => {
                        let name = CString::new(name).unwrap();
                        unsafe { AdsGetIndexHandle(table, name.as_ptr() as *mut _, &mut order) }
                    }
                };
                if rc == AE_SUCCESS {
                    orders[current - 1] = order;
                }
                rc
            }
            (None, "rows" | "count") => {
                let order = orders[current - 1];
                let moving = if order != 0 { order } else { table };
                let rc = if cmd == "rows" {
                    AdsGotoTop(moving)
                } else {
                    AE_SUCCESS
                };
                if cmd == "rows" {
                    let mut rows = Vec::new();
                    while rows.len() < 100_000 && rc == AE_SUCCESS {
                        let (mut eof, mut recno) = (1, 0);
                        unsafe { AdsAtEOF(table, &mut eof) };
                        if eof != 0 {
                            break;
                        }
                        unsafe { AdsGetRecordNum(table, ADS_IGNOREFILTERS, &mut recno) };
                        rows.push(recno);
                        if AdsSkip(moving, 1) != AE_SUCCESS {
                            break;
                        }
                    }
                    result["rows"] = json!(rows);
                }
                let mut n = 0;
                let count = unsafe { AdsGetRecordCount(table, ADS_RESPECTFILTERS, &mut n) };
                result["count"] = json!([count, n]);
                if order != 0 {
                    let keys = unsafe { AdsGetKeyCount(order, ADS_RESPECTFILTERS, &mut n) };
                    result["keys"] = json!([keys, n]);
                }
                rc
            }
            (None, "column") => {
                let order = orders[current - 1];
                let moving = if order != 0 { order } else { table };
                let field = CString::new(rest.trim()).unwrap();
                let rc = AdsGotoTop(moving);
                let mut values = Vec::new();
                while values.len() < 100_000 && rc == AE_SUCCESS {
                    let mut eof = 1;
                    unsafe { AdsAtEOF(table, &mut eof) };
                    if eof != 0 {
                        break;
                    }
                    let mut buffer = vec![0u8; 70_000];
                    let mut len = buffer.len() as u32;
                    let got = unsafe {
                        AdsGetField(table, field.as_ptr() as _, buffer.as_mut_ptr(), &mut len, 0)
                    };
                    values.push(match got {
                        AE_SUCCESS => json!(String::from_utf8_lossy(&buffer[..len as usize])),
                        _ => Json::Null,
                    });
                    if AdsSkip(moving, 1) != AE_SUCCESS {
                        break;
                    }
                }
                result["values"] = json!(values);
                rc
            }
            (None, "reindex61") => crate::AdsReindex61(table, rest.trim().parse().unwrap()),
            (None, "pack120") => {
                let size = word(&mut rest).parse().unwrap();
                crate::AdsPackTable120(table, size, rest.trim().parse().unwrap())
            }
            (None, "conn") => {
                let f = word(&mut rest);
                let flag: Option<u16> = rest.trim().parse().ok();
                match (f, flag) {
                    ("AdsSetFlushFlag", Some(flag)) => AdsSetFlushFlag(connection, flag),
                    ("AdsDisableRI", None) => AdsDisableRI(connection),
                    ("AdsEnableRI", None) => AdsEnableRI(connection),
                    ("AdsDisableUniqueEnforcement", None) => {
                        AdsDisableUniqueEnforcement(connection)
                    }
                    ("AdsEnableUniqueEnforcement", None) => AdsEnableUniqueEnforcement(connection),
                    ("AdsDisableAutoIncEnforcement", None) => {
                        AdsDisableAutoIncEnforcement(connection)
                    }
                    ("AdsEnableAutoIncEnforcement", None) => {
                        AdsEnableAutoIncEnforcement(connection)
                    }
                    other => panic!("unknown connection call {other:?}"),
                }
            }
            (None, "setbin") => {
                let field = FieldArg::new(word(&mut rest));
                let kind = word(&mut rest).parse().unwrap();
                let total = number(word(&mut rest));
                let offset = number(word(&mut rest));
                let mut bytes = from_hex(rest);
                unsafe {
                    AdsSetBinary(
                        table,
                        field.ptr(),
                        kind,
                        total,
                        offset,
                        bytes.as_mut_ptr(),
                        bytes.len() as u32,
                    )
                }
            }
            (None, "getopt") => {
                let field = FieldArg::new(word(&mut rest));
                let option = number(word(&mut rest)) as u16;
                let mut len = if rest.is_empty() { 200 } else { number(rest) };
                let mut buf = vec![b'#'; 70000];
                let rc =
                    unsafe { AdsGetField(table, field.ptr(), buf.as_mut_ptr(), &mut len, option) };
                result["len"] = json!(len);
                let shown = if len < 600 { len as usize } else { 0 };
                result["value"] = json!(hex(&buf[..shown]));
                rc
            }
            (None, "handletype") => {
                let mut kind = 77;
                let mut object = table;
                if !rest.is_empty() {
                    let name = in_folder(rest);
                    object = 0;
                    let rc = unsafe {
                        AdsGetIndexHandle(table, name.as_ptr() as *mut Unsigned8, &mut object)
                    };
                    result["order"] = json!(rc);
                }
                let rc = unsafe { AdsGetHandleType(object, &mut kind) };
                result["type"] = json!(kind);
                rc
            }
            (None, "lasterror") => {
                let (mut code, mut buf, mut len) = (0, [0u8; 1200], 1200u16);
                unsafe { crate::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
                result["code"] = json!(code);
                let shown = if code == 0 { 0 } else { usize::from(len) };
                result["text"] = json!(data_folder(&crate::ansi::to_string(&buf[..shown]), work));
                AE_SUCCESS
            }
            (None, "links") => {
                let mut links = 77;
                let rc = unsafe { AdsGetNumActiveLinks(connection, &mut links) };
                result["links"] = json!(links);
                rc
            }
            (None, "transfree") => {
                let rc = crate::AdsSetTableTransactionFree(table, rest.trim().parse().unwrap());
                // As write.c reads it: before the call after.
                if rc != 0 {
                    result["error"] = json!(last_error_text(work));
                }
                let mut free = 77;
                let free_rc = unsafe { AdsIsTableTransactionFree(table, &mut free) };
                result["free"] = json!([free_rc, free]);
                rc
            }
            (None, "keycolumn") => {
                let mut buf = [0u8; 512];
                let mut len: u16 = rest.trim().parse().unwrap_or(512);
                let rc = unsafe { AdsGetKeyColumn(table, buf.as_mut_ptr(), &mut len) };
                result["len"] = json!(len);
                let shown = if rc == AE_SUCCESS {
                    usize::from(len)
                } else {
                    0
                };
                result["value"] = json!(String::from_utf8_lossy(&buf[..shown]));
                rc
            }
            (None, "lookup") => {
                let tag = CString::new(word(&mut rest)).unwrap();
                let data_type: u16 = word(&mut rest).parse().unwrap();
                let mut key = match data_type {
                    1 => from_hex(rest),
                    4 => rest.trim().parse::<f64>().unwrap().to_le_bytes().to_vec(),
                    _ => unescape(rest),
                };
                let (mut order, mut found) = (0, 77);
                let mut rc =
                    unsafe { AdsGetIndexHandle(table, tag.as_ptr() as *mut _, &mut order) };
                if rc == AE_SUCCESS {
                    rc = unsafe {
                        AdsLookupKey(
                            order,
                            key.as_mut_ptr(),
                            key.len() as u16,
                            data_type,
                            &mut found,
                        )
                    };
                }
                result["found"] = json!(found);
                rc
            }
            (None, "keyfromrecord") => {
                let tag = CString::new(word(&mut rest)).unwrap();
                let mut record = vec![0u8; 65536];
                let mut len = record.len() as u32;
                if rest.is_empty() {
                    unsafe { AdsGetRecord(table, record.as_mut_ptr(), &mut len) };
                } else {
                    record = from_hex(rest);
                    len = record.len() as u32;
                }
                let (mut key, mut key_len) = ([0u8; 1024], 1024u16);
                let mut order = 0;
                let mut rc =
                    unsafe { AdsGetIndexHandle(table, tag.as_ptr() as *mut _, &mut order) };
                if rc == AE_SUCCESS {
                    rc = unsafe {
                        AdsBuildKeyFromRecord(
                            order,
                            record.as_mut_ptr(),
                            len,
                            key.as_mut_ptr(),
                            &mut key_len,
                        )
                    };
                }
                let shown = if rc == AE_SUCCESS {
                    usize::from(key_len)
                } else {
                    0
                };
                result["key"] = json!(hex(&key[..shown]));
                rc
            }
            (None, "setrecord") => {
                let mut record = from_hex(word(&mut rest));
                let len = rest.trim().parse().unwrap_or(record.len() as u32);
                unsafe { AdsSetRecord(table, record.as_mut_ptr(), len) }
            }
            (None, "fieldraw") => {
                let field = FieldArg::new(word(&mut rest));
                let mut bytes = from_hex(rest);
                unsafe {
                    AdsSetFieldRaw(table, field.ptr(), bytes.as_mut_ptr(), bytes.len() as u32)
                }
            }
            (None, "vfpnull") => {
                let field = FieldArg::new(word(&mut rest));
                let options = rest.trim().parse().unwrap();
                let mut record = vec![0u8; 65536];
                let mut len = record.len() as u32;
                unsafe { AdsGetRecord(table, record.as_mut_ptr(), &mut len) };
                let mut flag = 77;
                let rc = unsafe {
                    AdsAccessVfpSystemField(
                        table,
                        field.ptr(),
                        record.as_mut_ptr(),
                        options,
                        &mut flag,
                    )
                };
                result["flag"] = json!(flag);
                rc
            }
            (None, "rawkey") => {
                let tag = CString::new(word(&mut rest)).unwrap();
                let options = word(&mut rest).parse().unwrap();
                let (mut key, mut key_len) = ([0u8; 1024], 1024u16);
                let mut order = 0;
                let mut rc =
                    unsafe { AdsGetIndexHandle(table, tag.as_ptr() as *mut _, &mut order) };
                if rc == AE_SUCCESS {
                    rc = AdsInitRawKey(order);
                }
                for item in rest.split(';').filter(|item| !item.is_empty()) {
                    if rc != AE_SUCCESS {
                        break;
                    }
                    let Some((name, value)) = item.split_once('=') else {
                        continue;
                    };
                    let field = FieldArg::new(name);
                    let mut value = unescape(value);
                    rc = match value.is_empty() {
                        true => unsafe { AdsSetEmpty(order, field.ptr()) },
                        false => unsafe {
                            AdsSetField(order, field.ptr(), value.as_mut_ptr(), value.len() as u32)
                        },
                    };
                }
                if rc == AE_SUCCESS {
                    rc = unsafe {
                        AdsBuildRawKey100(order, key.as_mut_ptr(), &mut key_len, options)
                    };
                }
                let shown = if rc == AE_SUCCESS {
                    usize::from(key_len)
                } else {
                    0
                };
                result["key"] = json!(hex(&key[..shown]));
                rc
            }
            (None, "stepkey") => {
                let tag = CString::new(word(&mut rest)).unwrap();
                let direction: i16 = word(&mut rest).parse().unwrap();
                let mut key = from_hex(rest);
                let mut order = 0;
                let mut rc =
                    unsafe { AdsGetIndexHandle(table, tag.as_ptr() as *mut _, &mut order) };
                if rc == AE_SUCCESS {
                    rc = unsafe {
                        AdsStepIndexKey(order, key.as_mut_ptr(), key.len() as u16, direction)
                    };
                }
                result["key"] = json!(hex(&key));
                rc
            }
            (None, "locate") => {
                let forward: u16 = word(&mut rest).parse().unwrap();
                let mut text = unescape(rest);
                text.push(0);
                let mut found = 77;
                let rc = unsafe { crate::AdsLocate(table, text.as_mut_ptr(), forward, &mut found) };
                result["found"] = json!(found);
                rc
            }
            (None, "continue") => {
                let mut found = 77;
                let rc = unsafe { crate::AdsContinue(table, &mut found) };
                result["found"] = json!(found);
                rc
            }
            (None, "filetobinary") => {
                let field = FieldArg::new(word(&mut rest));
                let binary_type: u16 = word(&mut rest).parse().unwrap();
                // The file is the client's, in the scenario's copy.
                let path = CString::new(work.join(rest).to_str().unwrap()).unwrap();
                unsafe { AdsFileToBinary(table, field.ptr(), binary_type, path.as_ptr() as *mut _) }
            }
            (None, "cpstring") => {
                let field = FieldArg::new(word(&mut rest));
                let code_page = word(&mut rest).parse().unwrap();
                let mut text = unescape(rest);
                unsafe {
                    AdsSetStringFromCodePage(
                        table,
                        code_page,
                        field.ptr(),
                        text.as_mut_ptr(),
                        text.len() as u32,
                    )
                }
            }
            (None, "addcustom" | "delcustom") => {
                let tag = CString::new(rest).unwrap();
                let mut order = 0;
                let mut rc =
                    unsafe { AdsGetIndexHandle(table, tag.as_ptr() as *mut _, &mut order) };
                if rc == AE_SUCCESS {
                    rc = match cmd {
                        "addcustom" => AdsAddCustomKey(order),
                        _ => AdsDeleteCustomKey(order),
                    };
                }
                rc
            }
            (None, "copytable" | "copystructure") => {
                let from = match orders[current - 1] {
                    0 => table,
                    order => order,
                };
                if cmd == "copytable" {
                    let filter = word(&mut rest).parse().unwrap();
                    let file = in_folder(rest);
                    unsafe { crate::AdsCopyTable(from, filter, file.as_ptr() as *mut _) }
                } else {
                    let file = in_folder(rest);
                    unsafe { crate::AdsCopyTableStructure(table, file.as_ptr() as *mut _) }
                }
            }
            (None, "convert") => {
                let from = match orders[current - 1] {
                    0 => table,
                    order => order,
                };
                let filter = word(&mut rest).parse().unwrap();
                let table_type = match word(&mut rest) {
                    "ntx" => ADS_NTX,
                    "cdx" => ADS_CDX,
                    "vfp" => ADS_VFP,
                    _ => ADS_ADT,
                };
                let file = in_folder(rest);
                unsafe { crate::AdsConvertTable(from, filter, file.as_ptr() as *mut _, table_type) }
            }
            (None, "copycontents") => {
                let from = match orders[current - 1] {
                    0 => table,
                    order => order,
                };
                let to: usize = word(&mut rest).parse().unwrap();
                crate::AdsCopyTableContents(from, tables[to - 1], rest.trim().parse().unwrap())
            }
            (None, "restructure") => {
                let table_name = word(&mut rest);
                // A dictionary's table is given by its name.
                let file = match table_name.strip_prefix(':') {
                    Some(name) => CString::new(name).unwrap(),
                    None => in_folder(table_name),
                };
                let block_size = word(&mut rest).parse().unwrap();
                let mut lists = rest.splitn(3, '|').map(|l| CString::new(l).unwrap());
                let mut list = || lists.next().unwrap_or_default();
                let (add, delete, change) = (list(), list(), list());
                unsafe {
                    crate::AdsRestructureTable120(
                        connection,
                        file.as_ptr() as *mut _,
                        std::ptr::null_mut(),
                        table_type,
                        char_type,
                        ADS_PROPRIETARY_LOCKING,
                        1,
                        add.as_ptr() as *mut _,
                        delete.as_ptr() as *mut _,
                        change.as_ptr() as *mut _,
                        c"".as_ptr() as *mut _,
                        block_size,
                        0,
                    )
                }
            }
            (None, cmd) => panic!("unknown command {cmd}"),
        };
        result["rc"] = json!(rc);
        if rc != 0 && result.get("error").is_none() {
            result["error"] = json!(last_error_text(work));
        }
        results.push(result);
    }
    if statement != 0 {
        AdsCloseSQLStatement(statement);
    }
    AdsDisconnect(connection);
    if connection2 != 0 {
        AdsDisconnect(connection2);
    }
    (results, snapshots)
}

/// A number of a script, decimal or `0x` hex, as C's `strtoul(.., 0)` reads it.
fn number(text: &str) -> u32 {
    match text.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).unwrap(),
        None => text.trim().parse().unwrap(),
    }
}

/// AdsGetLastError's text, as write.c records it (see [`data_folder`]).
fn last_error_text(work: &Path) -> String {
    let (mut code, mut buf, mut len) = (0, [0u8; 1200], 1200u16);
    unsafe { crate::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
    data_folder(&crate::ansi::to_string(&buf[..usize::from(len)]), work)
}

/// `text` with the data folder `work` written as `<data>` and the paths in
/// it with backslashes, as `write.c` writes the original engine's.
fn data_folder(text: &str, work: &Path) -> String {
    let folder = work.display().to_string();
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(&folder) {
        out.push_str(&rest[..at]);
        out.push_str("<data>");
        rest = &rest[at + folder.len()..];
        let end = rest.find([' ', '"']).unwrap_or(rest.len());
        out.push_str(&rest[..end].replace('/', "\\"));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// The bytes of `path` that hold the time of a write: the ModTime fields of
/// an ADT table, the date of the last update in a DBF table's header;
/// (offset, length) pairs in the file.
fn modtime_bytes(path: &Path) -> Vec<(usize, usize)> {
    // A DBF table's, and its backup's (`.dbf.BAK`, from a restructure).
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase());
    if name.is_some_and(|n| n.ends_with(".dbf") || n.ends_with(".dbf.bak")) {
        return vec![(1, 3)];
    }
    let Ok(structure) = Structure::read(path, Format::Adt) else {
        return Vec::new();
    };
    let header = &structure.header;
    let size = std::fs::metadata(path).unwrap().len() as usize;
    let start = header.data_offset as usize;
    let length = header.record_length as usize;
    let mut ranges = Vec::new();
    for slot in 0..size.saturating_sub(start) / length {
        for field in structure
            .fields
            .iter()
            .filter(|f| f.field_type == FieldType::ModTime)
        {
            ranges.push((
                start + slot * length + usize::from(field.offset),
                usize::from(field.size),
            ));
        }
    }
    ranges
}

/// The bytes of the ADT file `path` that hold the number of a transaction
/// that marked a record: the byte after the status of each marked record
/// (see `tflip_storage::adt::TX_MARK`), which is the server's own number.
fn tx_mark_bytes(path: &Path) -> Vec<(usize, usize)> {
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("adt"))
    {
        return Vec::new();
    }
    let Ok(structure) = Structure::read(path, Format::Adt) else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let (start, length) = (
        structure.header.data_offset as usize,
        structure.header.record_length as usize,
    );
    let mut ranges = Vec::new();
    let mut at = start;
    while at + length <= bytes.len() {
        let status = bytes[at];
        if status & tflip_storage::adt::TX_MARK != 0 || status == tflip_storage::adt::TX_APPENDED {
            ranges.push((at + 1, 1));
        }
        at += length;
    }
    ranges
}

/// The bytes of the NTX file `path` that hold nothing: on each page, all
/// but its key count, its offset table, its items and the child pointer of
/// the item that holds its rightmost child. The original engine leaves there
/// what its page buffers held; (offset, length) pairs in the file.
fn ntx_unused_bytes(path: &Path) -> Vec<(usize, usize)> {
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("ntx"))
    {
        return Vec::new();
    }
    let bytes = std::fs::read(path).unwrap();
    let u16_at = |at: usize| usize::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]));
    let item_size = u16_at(0x0C);
    let max = u16_at(0x12);
    let mut ranges = Vec::new();
    for page in (1024..bytes.len()).step_by(1024) {
        let n = u16_at(page).min(max);
        let mut used = vec![false; 1024];
        used[..2 + 2 * (max + 1)].fill(true);
        for i in 0..=n {
            let slot = u16_at(page + 2 + 2 * i);
            let length = if i < n { item_size } else { 4 };
            if let Some(range) = used.get_mut(slot..slot + length) {
                range.fill(true);
            }
        }
        let mut at = 0;
        while at < 1024 {
            let run = used[at..].iter().take_while(|&&u| u == used[at]).count();
            if !used[at] {
                ranges.push((page + at, run));
            }
            at += run;
        }
    }
    ranges
}

/// The byte ranges `volatile` (a `volatile.json`) lists for `file`.
fn volatile_ranges<'v>(
    volatile: &'v serde_json::Map<String, Json>,
    file: &str,
) -> impl Iterator<Item = (usize, usize)> + 'v {
    volatile.get(file).into_iter().flat_map(|ranges| {
        ranges.as_array().unwrap().iter().map(|range| {
            let at = |i: usize| range[i].as_u64().unwrap() as usize;
            (at(0), at(1))
        })
    })
}

/// What differs between the original's file `path`, holding `original`, and
/// `written`, but for the bytes `masked` and those that hold the time of a
/// write or nothing; `None` if nothing does.
pub(crate) fn compare(
    path: &Path,
    mut original: Vec<u8>,
    mut written: Vec<u8>,
    masked: Vec<(usize, usize)>,
) -> Option<String> {
    let masked = modtime_bytes(path)
        .into_iter()
        .chain(ntx_unused_bytes(path))
        .chain(tx_mark_bytes(path))
        .chain(masked);
    for (offset, length) in masked {
        for bytes in [&mut original, &mut written] {
            if let Some(range) = bytes.get_mut(offset..offset + length) {
                range.fill(0);
            }
        }
    }
    if written == original {
        return None;
    }
    let at = written.iter().zip(&original).position(|(a, b)| a != b);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    // The bytes there, of both.
    let near = |bytes: &[u8]| {
        let start = at.unwrap_or(0).saturating_sub(4);
        let end = (start + 16).min(bytes.len());
        bytes
            .get(start..end)
            .map(|b| b.iter().map(|b| format!("{b:02x}")).collect::<String>())
    };
    Some(format!(
        "  {name} differs: {} bytes, the original's {}, first difference at {at:?} \
         (tflip {:?}, the original {:?})",
        written.len(),
        original.len(),
        near(&written).unwrap_or_default(),
        near(&original).unwrap_or_default()
    ))
}

/// Runs scenario `name` and checks it against the original's results.
fn check(name: &str) {
    check_dir(name, &scenario_dir(name));
}

/// A scenario recorded in the folder that `TFLIP_WRITE_SCENARIO` names, laid
/// out as those of `fixtures/write/` (`tools/ace/write/run.sh` makes one), to
/// try one out while working on it.
#[test]
#[ignore]
fn scenario_from_env() {
    let dir = PathBuf::from(std::env::var("TFLIP_WRITE_SCENARIO").expect("TFLIP_WRITE_SCENARIO"));
    check_dir("scenario", &dir);
}

/// Runs the scenario in `dir` and checks it against the original's results.
fn check_dir(name: &str, dir: &Path) {
    let dir = dir.to_path_buf();
    let (source, work) = fixtures::scratch_from(&format!("write-{name}"), &dir.join("before"));
    let script = std::fs::read_to_string(dir.join("ops.txt")).unwrap();
    // A scenario that changes whether deleted records show changes it for
    // the process: the others must not run meanwhile.
    // So does one that ran under a static collation (`# ANSI_CHAR_SET=`, the
    // original engine's adslocal.cfg setting).
    let ansi = script
        .lines()
        .find_map(|line| line.strip_prefix("# ANSI_CHAR_SET="));
    // `AdsWriteAllRecords` writes the records every test has pending, which a
    // constrained cursor's refused one fails.
    let alone = ansi.is_some()
        || script.lines().any(|line| {
            ["showdeleted", "exact", "writeall", "constrain"]
                .iter()
                .any(|command| line.starts_with(command))
        });
    let (_alone, _shared) = match alone {
        true => (Some(fixtures::settings_lock()), None),
        false => (None, Some(fixtures::settings_shared())),
    };
    if let (Some(ansi), Some(collation_dir)) = (ansi, crate::config::collation_dir()) {
        let collations = tflip_engine::collations::Collations::new(&collation_dir, ansi.trim(), "");
        tflip_engine::settings::set_collations(&crate::LOCAL, collations);
    }
    // The bytes the original engine leaves uninitialised, which differed
    // between two runs of the scenario (tools/ace/write/run.sh).
    let volatile: serde_json::Map<String, Json> =
        std::fs::read_to_string(dir.join("volatile.json"))
            .map(|text| serde_json::from_str(&text).unwrap())
            .unwrap_or_default();
    tflip_storage::undefined::take(&work);
    let (got, snapshots) = replay(&source, &work, &script);
    // Each probe ran in a process of its own: what the scenario changed of
    // the process goes back to the default for the next one.
    if _alone.is_some() {
        crate::AdsShowDeleted(crate::ADS_TRUE);
        crate::settings::AdsSetExact(crate::ADS_FALSE);
    }
    if ansi.is_some() {
        tflip_engine::settings::set_collations(&crate::LOCAL, crate::local_collations());
    }
    // The bytes tflip wrote where the original leaves its memory's.
    let undefined = tflip_storage::undefined::take(&work);
    let expected: Vec<Json> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("result.json")).unwrap()).unwrap();
    assert_eq!(got.len(), expected.len(), "{name}: number of commands");
    // The answers that differ for reasons KNOWN_ISSUES.md lists, by command:
    // {"<number>": {"fields": [...], "why": "..."}}; those fields are not
    // compared.
    let known: serde_json::Map<String, Json> = std::fs::read_to_string(dir.join("known.json"))
        .map(|text| serde_json::from_str(&text).unwrap())
        .unwrap_or_default();
    let mut problems = Vec::new();
    for (i, (mut got, mut expected)) in got.into_iter().zip(expected).enumerate() {
        // AdsGetLastError's text, with the details the original engine adds
        // (the field, the file, ...).
        let want_error = expected.as_object_mut().unwrap().remove("error");
        let got_error = got.as_object_mut().unwrap().remove("error");
        let fields = known
            .get(&i.to_string())
            .and_then(|k| k["fields"].as_array());
        // A known difference in the code is one in the text too.
        let text_known = fields.is_some_and(|f| f.iter().any(|f| f == "error" || f == "rc"));
        if !text_known && want_error.is_some() && want_error != got_error {
            let (want, got) = (
                want_error.unwrap_or_default(),
                got_error.unwrap_or_default(),
            );
            problems.push(format!("  #{i} error text {got}\n  expected {want}"));
        }
        if let Some(fields) = fields {
            for field in fields.iter().filter_map(Json::as_str) {
                got.as_object_mut().unwrap().remove(field);
                expected.as_object_mut().unwrap().remove(field);
            }
        }
        if got != expected {
            problems.push(format!("  #{i} got {got}\n  expected {expected}"));
        }
    }
    let mut changed = false;
    // Files a scenario says it does not compare, for a difference
    // KNOWN_ISSUES lists: `# Not compared: <file>[, <file>]`.
    let skipped: Vec<&str> = script
        .lines()
        .filter_map(|line| line.strip_prefix("# Not compared: "))
        .flat_map(|files| files.split(','))
        .map(str::trim)
        .collect();
    for entry in std::fs::read_dir(dir.join("after")).unwrap() {
        let entry = entry.unwrap();
        let file = entry.file_name();
        if skipped.contains(&&*file.to_string_lossy()) {
            continue;
        }
        let original = std::fs::read(entry.path()).unwrap();
        changed |= std::fs::read(dir.join("before").join(&file)).ok().as_ref() != Some(&original);
        let Ok(written) = std::fs::read(work.join(&file)) else {
            problems.push(format!("  {} was not written", file.display()));
            continue;
        };
        let ranges = volatile_ranges(&volatile, &file.to_string_lossy());
        let undefined = undefined
            .iter()
            .filter(|(path, _)| path.file_name() == Some(&*file));
        let undefined = undefined.map(|&(_, offset)| (offset as usize, 1));
        let masked = ranges.chain(undefined).collect();
        if let Some(problem) = compare(&entry.path(), original, written, masked) {
            problems.push(problem);
        }
    }
    // The files in the middle of the script (`snapshot`): the tables, as the
    // original's index and memo files there are not what the server holds
    // (it caches index pages, and keeps its memo files' free lists in
    // memory while a transaction runs); all of them in a scenario whose
    // snapshots are taken with its tables closed, which says so.
    let closed = script
        .lines()
        .any(|line| line.starts_with("# Snapshots of closed files"));
    let snaps_volatile: serde_json::Map<String, Json> =
        std::fs::read_to_string(dir.join("snaps-volatile.json"))
            .map(|text| serde_json::from_str(&text).unwrap())
            .unwrap_or_default();
    for (snap, files) in snapshots {
        let folder = dir.join("snaps").join(&snap);
        for (file, written) in files {
            let path = folder.join(&file);
            let index = path.extension().is_some_and(|e| {
                ["adi", "cdx", "ntx", "idx", "adm", "fpt", "dbt"]
                    .iter()
                    .any(|x| e.eq_ignore_ascii_case(x))
            });
            if index && !closed {
                continue;
            }
            let Ok(original) = std::fs::read(&path) else {
                problems.push(format!(
                    "  {snap}/{}: the original has no such file",
                    file.display()
                ));
                continue;
            };
            let key = format!("{snap}/{}", file.to_string_lossy());
            let masked = volatile_ranges(&snaps_volatile, &key).collect();
            if let Some(problem) = compare(&path, original, written, masked) {
                problems.push(format!("  {snap}/{}", problem.trim_start()));
            }
        }
    }
    // A scenario that only reads says so in a comment.
    let reads_only = script.lines().any(|line| line.starts_with("# Reads only"));
    assert!(
        changed || reads_only,
        "{name}: the scenario changes no file"
    );
    assert!(problems.is_empty(), "{name}:\n{}", problems.join("\n"));
}

#[test]
fn records() {
    check("records");
}

#[test]
fn aofs() {
    check("aofs");
}

#[test]
fn aofs_lookup() {
    check("aofs_lookup");
}

#[test]
fn aofs_more() {
    check("aofs_more");
}

/// Has a table under a dynamic collation: needs the original engine's
/// collation files (see [`crate::config::collation_dir`]); skipped without
/// them.
#[test]
fn aofs_rest() {
    let dir = crate::config::collation_dir();
    if dir.is_none_or(|dir| !dir.join("adscollate.adt").exists()) {
        return;
    }
    check("aofs_rest");
}

/// Under the static collation `ANSI_CHAR_SET=German`; needs the original
/// engine's collation files (see [`crate::config::collation_dir`]), skipped
/// without them.
#[test]
fn aofs_static() {
    let dir = crate::config::collation_dir();
    if dir.is_none_or(|dir| !dir.join("ansi.chr").exists()) {
        return;
    }
    check("aofs_static");
}

/// Under `ANSI_CHAR_SET=German` too (see [`aofs_static`]).
#[test]
fn aofs_static_keys() {
    let dir = crate::config::collation_dir();
    if dir.is_none_or(|dir| !dir.join("ansi.chr").exists()) {
        return;
    }
    check("aofs_static_keys");
}

/// Needs the original engine's collation files (see
/// [`crate::config::collation_dir`]); skipped without them.
#[test]
fn aofs_collation() {
    let dir = crate::config::collation_dir();
    if dir.is_none_or(|dir| !dir.join("adscollate.adt").exists()) {
        return;
    }
    check("aofs_collation");
}

#[test]
fn ddl() {
    check("ddl");
}

#[test]
fn build() {
    check("build");
}

#[test]
fn fts_build() {
    check("fts_build");
}

#[test]
fn fts_rebuild() {
    check("fts_rebuild");
}

#[test]
fn fts_writes() {
    check("fts_writes");
}

#[test]
fn fts_tx_rollback() {
    check("fts_tx_rollback");
}

#[test]
fn fts_tx_commit() {
    check("fts_tx_commit");
}

#[test]
fn fts_tx_big_rollback() {
    check("fts_tx_big_rollback");
}

#[test]
fn fts_tx_big_commit() {
    check("fts_tx_big_commit");
}

#[test]
fn fts_tx_memo_rollback() {
    check("fts_tx_memo_rollback");
}

#[test]
fn fts_tx_memo_commit() {
    check("fts_tx_memo_commit");
}

#[test]
fn fts_tx_cdx_rollback() {
    check("fts_tx_cdx_rollback");
}

#[test]
fn fts_tx_cdx_commit() {
    check("fts_tx_cdx_commit");
}

#[test]
fn fts_tx_vfp_rollback() {
    check("fts_tx_vfp_rollback");
}

#[test]
fn fts_tx_vfp_commit() {
    check("fts_tx_vfp_commit");
}

#[test]
fn fts_tx_exclusive() {
    check("fts_tx_exclusive");
}

#[test]
fn fts_binary() {
    check("fts_binary");
}

#[test]
fn fts_recall() {
    check("fts_recall");
}

#[test]
fn fts_recall_binary() {
    check("fts_recall_binary");
}

#[test]
fn fts_cdx_create() {
    check("fts_cdx_create");
}

#[test]
fn fts_cdx() {
    check("fts_cdx");
}

#[test]
fn fts_cdx_null() {
    check("fts_cdx_null");
}

#[test]
fn maintenance() {
    check("maintenance");
}

#[test]
fn table_ops() {
    check("table_ops");
}

#[test]
fn locate_custom() {
    check("locate_custom");
}

#[test]
fn restructure() {
    check("restructure");
}

#[test]
fn restructure_rename() {
    check("restructure_rename");
}

#[test]
fn bitmaps() {
    check("bitmaps");
}

#[test]
fn cdx_bitmaps() {
    check("cdx_bitmaps");
}

#[test]
fn bitmap_split() {
    check("bitmap_split");
}

#[test]
fn bitmap_levels() {
    check("bitmap_levels");
}

#[test]
fn cdx_bitmap_split() {
    check("cdx_bitmap_split");
}

#[test]
fn cdx_bitmap_levels() {
    check("cdx_bitmap_levels");
}

#[test]
fn dbf_create() {
    check("dbf_create");
}

#[test]
fn ntx_create() {
    check("ntx_create");
}

#[test]
fn sql_dbf() {
    check("sql_dbf");
}

#[test]
fn cdx_drop() {
    check("cdx_drop");
}

#[test]
fn tx_basic() {
    check("tx_basic");
}

#[test]
fn tx_multi() {
    check("tx_multi");
}

/// Two handles of one table on one connection lock separately: a record one
/// has locked, or wrote in a transaction, the other cannot lock or write.
#[test]
fn tx_handles() {
    check("tx_handles");
}

#[test]
fn tx_abandon() {
    check("tx_abandon");
}

#[test]
fn tx_dbf() {
    check("tx_dbf");
}

#[test]
fn deleted_cursor() {
    check("deleted_cursor");
}

#[test]
fn set_errors() {
    check("set_errors");
}

#[test]
fn goto_deleted() {
    check("goto_deleted");
}

#[test]
fn skip_unkeyed() {
    check("skip_unkeyed");
}

#[test]
fn skip_deleted() {
    check("skip_deleted");
}

#[test]
fn vfp_timestamps() {
    check("vfp_timestamps");
}

#[test]
fn cdx_timestamps() {
    check("cdx_timestamps");
}

#[test]
fn error_details() {
    check("error_details");
}

#[test]
fn open_opened() {
    check("open_opened");
}

#[test]
fn database_type() {
    check("database_type");
}

#[test]
fn relations() {
    check("relations");
}

#[test]
fn relations_dbf() {
    check("relations_dbf");
}

#[test]
fn tx_pending() {
    check("tx_pending");
}

#[test]
fn tx_append() {
    check("tx_append");
}

#[test]
fn tx_sql() {
    check("tx_sql");
}

#[test]
fn tx_vfp() {
    check("tx_vfp");
}

#[test]
fn tx_dbf_memo() {
    check("tx_dbf_memo");
}

#[test]
fn tx_memo_exclusive() {
    check("tx_memo_exclusive");
}

#[test]
fn tx_memo_sizes() {
    check("tx_memo_sizes");
}

#[test]
fn tx_savepoint_lock() {
    check("tx_savepoint_lock");
}

#[test]
fn tx_savepoint_memo() {
    check("tx_savepoint_memo");
}

#[test]
fn tx_redelete() {
    check("tx_redelete");
}

#[test]
fn tx_cursor_after() {
    check("tx_cursor_after");
}

#[test]
fn memo_other_handle() {
    check("memo_other_handle");
}

#[test]
fn cdx_create() {
    check("cdx_create");
}

#[test]
fn limits() {
    check("limits");
}

#[test]
fn tagdir() {
    check("tagdir");
}

#[test]
fn tagdir_drop() {
    check("tagdir_drop");
}

#[test]
fn dbf_maintenance() {
    check("dbf_maintenance");
}

#[test]
fn dbf_rebuild() {
    check("dbf_rebuild");
}

#[test]
fn dbf_recall() {
    check("dbf_recall");
}

/// The RC4 encryption calls on free Clipper, FoxPro and Visual FoxPro tables
/// (`tools/ace/write/dbf_encryption.py`).
#[test]
fn dbf_encryption_ntx() {
    check("dbf_encryption_ntx");
}

#[test]
fn dbf_encryption_cdx() {
    check("dbf_encryption_cdx");
}

#[test]
fn dbf_encryption_vfp() {
    check("dbf_encryption_vfp");
}

/// What the current record reads as after `AdsDisableEncryption`
/// (`tools/ace/write/dbf_encryption_cached.py`).
#[test]
fn dbf_encryption_cached_ntx() {
    check("dbf_encryption_cached_ntx");
}

#[test]
fn dbf_encryption_cached_cdx() {
    check("dbf_encryption_cached_cdx");
}

#[test]
fn dbf_encryption_cached_vfp() {
    check("dbf_encryption_cached_vfp");
}

/// The same on an ADT table, which reads the file again.
#[test]
fn dbf_encryption_cached_adt() {
    check("dbf_encryption_cached_adt");
}

/// An armed VFP table laid out for AES, opened without its password (free,
/// as a dictionary's table opened free, and an RC4 one for comparison): its
/// records read as zeros, appends and writes are 5096, a delete is accepted
/// and changes nothing (`fixtures/write/dbf_aes_nopassword`).
#[test]
fn dbf_aes_nopassword() {
    check("dbf_aes_nopassword");
}

/// Changes, appends and deletes in a transaction on encrypted tables
/// (`tools/ace/write/dbf_encryption_tx.py`, recorded on the remote server).
#[test]
fn dbf_encryption_tx() {
    check("dbf_encryption_tx");
}

#[test]
fn restructure_fts() {
    check("restructure_fts");
}

#[test]
fn restructure_fts_types() {
    check("restructure_fts_types");
}

#[test]
fn restructure_fts_layout() {
    check("restructure_fts_layout");
}

#[test]
fn restructure_fts_cdx() {
    check("restructure_fts_cdx");
}

#[test]
fn restructure_fts_vfp() {
    check("restructure_fts_vfp");
}

#[test]
fn restructure_fts_other() {
    check("restructure_fts_other");
}

#[test]
fn restructure_fts_other_cdx() {
    check("restructure_fts_other_cdx");
}

#[test]
fn restructure_fts_dict() {
    check("restructure_fts_dict");
}

#[test]
fn restructure_fts_dict_after() {
    check("restructure_fts_dict_after");
}

#[test]
fn restructure_fts_dict_after_fields() {
    check("restructure_fts_dict_after_fields");
}

#[test]
fn restructure_fts_dict_after_types() {
    check("restructure_fts_dict_after_types");
}

#[test]
fn restructure_fts_dict_after_keys() {
    check("restructure_fts_dict_after_keys");
}

/// The answers match; the dictionary's objects do not: where the original
/// puts them after a failed drop of an order's field is not worked out
/// (KNOWN_ISSUES, restructure after a failed one).
#[test]
#[ignore = "the dictionary's object order after a failed drop is not matched"]
fn restructure_fts_dict_drop() {
    check("restructure_fts_dict_drop");
}

#[test]
fn dbf_restructure() {
    check("dbf_restructure");
}

#[test]
fn cdx_kinds() {
    check("cdx_kinds");
}

#[test]
fn cdx_custom_reindex() {
    check("cdx_custom_reindex");
}

/// Deletes and recalls of ADT records against a binary order: what each one
/// counts in the tag header the next reindex writes, a delete not written
/// yet taken back, a deleted record gone to recalled, a written delete
/// leaving the table on no record.
/// The ADT free list in transactions: a slot deleted in one is not taken
/// by an append in it, a slot freed before is, and a rollback gives the
/// appended slots back in the order they were written.
#[test]
fn tx_free() {
    check("tx_free");
}

#[test]
fn binary_changes() {
    check("binary_changes");
}

/// Which deleted record an ADT append takes: the free list's head, none
/// where a recall left a stale link, a record gone back to by a cancelled
/// append, in a transaction and with another handle's lock.
#[test]
fn append_reuse() {
    check("append_reuse");
}

/// `AdsReindex61` with page sizes of every kind, valid and not.
#[test]
fn reindex_pages() {
    check("reindex_pages");
}

/// The next AutoInc value after a zap, a delete of every record, a pack and
/// a field made AutoInc.
#[test]
fn autoinc_reset() {
    check("autoinc_reset");
}

/// Writes through a live SQL cursor go to its table: append (AutoInc),
/// delete, recall, write, cancel and the locks, on ADT, with a column subset
/// and after the table's own writes.
#[test]
fn cursor_writes() {
    check("cursor_writes");
}

/// What a static cursor, a join's and a live one of a read-only statement
/// answer to the writes: 5072, or 5115 for the table-wide ones (locking,
/// pack, zap, reindex), which no cursor handle takes.
#[test]
fn cursor_refused() {
    check("cursor_refused");
}

/// A live cursor with a `WHERE` or an `ORDER BY` (an order's index, or a
/// temporary one): an appended record outside the `WHERE` stays, the order
/// keeps the records a write adds or moves.
#[test]
fn cursor_where() {
    check("cursor_where");
}

/// The same on a VFP table, whose deleted records stay shown.
#[test]
fn cursor_dbf() {
    check("cursor_dbf");
}

/// `AdsStmtConstrainUpdates`: a write that leaves the cursor's `WHERE` fails
/// with 7065 and stays pending.
#[test]
fn cursor_constrain() {
    check("cursor_constrain");
}

/// `AdsStmtConstrainUpdates` and `AdsStmtReadAllColumns`: their values, and
/// that neither takes an open cursor; a cursor's columns are what it names.
#[test]
fn cursor_settings() {
    check("cursor_settings");
}

/// A live cursor's locks and its view of another handle's writes.
#[test]
fn cursor_locks() {
    check("cursor_locks");
}

/// Cursors on a dictionary's tables and views: defaults, a view's columns
/// and aliases, a view of a view, constrained.
#[test]
fn cursor_views() {
    check("cursor_views");
}

/// The `AdsDisable`/`AdsEnable` Unique and AutoInc enforcement toggles: with
/// AutoInc enforcement off a client stores its own value and the next value is
/// kept past it; with unique enforcement off a duplicate key is written instead
/// of refused (`AE_KEY_NOT_UNIQUE`).
#[test]
fn enforce() {
    check("enforce");
}

#[test]
fn vfp_orders() {
    check("vfp_orders");
}

#[test]
fn copies() {
    check("copies");
}

#[test]
fn converts() {
    check("converts");
}

#[test]
fn converts_ext() {
    check("converts_ext");
}

/// `AdsConvertTable` of Clipper, FoxPro and Visual FoxPro tables of every
/// field type to each table type.
#[test]
fn converts_dbf() {
    check("converts_dbf");
}

/// `AdsConvertTable` of ADT tables of every field type to each table type:
/// NULL, empty and boundary values, deleted records, Double decimals, long
/// names, and values a converted field cannot hold.
#[test]
fn converts_values() {
    check("converts_values");
}

#[test]
fn setters() {
    check("setters");
}

#[test]
fn locks() {
    check("locks");
}

#[test]
fn memos() {
    check("memos");
}

#[test]
fn binary_pieces() {
    check("binary_pieces");
}

#[test]
fn binary_pieces_image() {
    check("binary_pieces_image");
}

#[test]
fn binary_pieces_records() {
    check("binary_pieces_records");
}

#[test]
fn guid_formats() {
    check("guid_formats");
}

#[test]
fn guid_formats_more() {
    check("guid_formats_more");
}

#[test]
fn handle_kinds() {
    check("handle_kinds");
}

#[test]
fn copy_made_by_client() {
    check("copy_made_by_client");
}

#[test]
fn memo_setters() {
    check("memo_setters");
}

#[test]
fn indexes() {
    check("indexes");
}

#[test]
fn nchar_indexes() {
    check("nchar_indexes");
}

#[test]
fn nchar_create() {
    check("nchar_create");
}

#[test]
fn uchar_keys() {
    check("uchar_keys");
}

#[test]
fn nchar_expr() {
    check("nchar_expr");
}

#[test]
fn nchar_expr_more() {
    check("nchar_expr_more");
}

#[test]
fn dbf_records() {
    check("dbf_records");
}

#[test]
fn dbf_memos() {
    check("dbf_memos");
}

#[test]
fn dbf_oem() {
    check("dbf_oem");
}

#[test]
fn ntx_indexes() {
    check("ntx_indexes");
}

#[test]
fn cdx_indexes() {
    check("cdx_indexes");
}

#[test]
fn dbf_setters() {
    check("dbf_setters");
}

remote_twins!(
    records,
    aofs,
    ddl,
    build,
    maintenance,
    table_ops,
    locate_custom,
    restructure,
    restructure_rename,
    restructure_fts,
    restructure_fts_types,
    restructure_fts_layout,
    restructure_fts_cdx,
    restructure_fts_vfp,
    restructure_fts_other,
    restructure_fts_other_cdx,
    bitmaps,
    cdx_bitmaps,
    bitmap_split,
    bitmap_levels,
    cdx_bitmap_split,
    cdx_bitmap_levels,
    dbf_create,
    ntx_create,
    cdx_create,
    limits,
    tagdir,
    tagdir_drop,
    dbf_maintenance,
    dbf_rebuild,
    dbf_recall,
    dbf_restructure,
    cdx_kinds,
    cdx_custom_reindex,
    vfp_orders,
    copies,
    converts,
    converts_ext,
    converts_dbf,
    converts_values,
    setters,
    locks,
    memos,
    memo_setters,
    copy_made_by_client,
    binary_pieces,
    binary_pieces_image,
    binary_pieces_records,
    guid_formats,
    guid_formats_more,
    indexes,
    dbf_records,
    dbf_setters,
    dbf_memos,
    dbf_oem,
    sql_dbf,
    cdx_drop,
    tx_basic,
    tx_multi,
    tx_handles,
    tx_abandon,
    tx_dbf,
    tx_pending,
    tx_append,
    tx_sql,
    tx_free,
    tx_vfp,
    tx_dbf_memo,
    tx_memo_exclusive,
    tx_memo_sizes,
    tx_savepoint_lock,
    tx_savepoint_memo,
    tx_redelete,
    tx_cursor_after,
    memo_other_handle,
    fts_tx_rollback,
    fts_tx_commit,
    fts_tx_big_rollback,
    fts_tx_big_commit,
    fts_tx_memo_rollback,
    fts_tx_memo_commit,
    fts_tx_cdx_rollback,
    fts_tx_cdx_commit,
    fts_tx_vfp_rollback,
    fts_tx_vfp_commit,
    fts_tx_exclusive,
    relations,
    relations_dbf,
    open_opened,
    database_type,
    goto_deleted,
    set_errors,
    deleted_cursor,
    skip_unkeyed,
    skip_deleted,
    vfp_timestamps,
    cdx_timestamps,
    error_details
);
