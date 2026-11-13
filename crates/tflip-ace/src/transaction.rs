//! Transactions (see [`tflip_engine::transaction`]): begun, committed and
//! rolled back on a connection, or with connection 0 on every connection of
//! the process, on the local server and on every server it is connected to.

use tflip_engine::Error;
use tflip_protocol::Api;

use crate::backend::{self, api};
use crate::ffi::text;
use crate::handle::{answer, connection_check};
use crate::last_error::{Call, fail, fail_text};
use crate::{
    AE_INVALID_POINTER, AE_NO_DRIVE_CONNECTION, AE_SUCCESS, AE_TRANS_OUT_OF_SEQUENCE, AdsHandle,
    LOCAL, Unsigned8, Unsigned16, Unsigned32, code, error_code,
};

/// Runs `f` on the backend of `connect`, or for 0 on every backend: then
/// it succeeds if it did on any (the others had no transaction), else fails
/// as the first did.
fn on(connect: AdsHandle, f: impl Fn(&dyn Api) -> Result<(), Error>) -> Unsigned32 {
    if connect != 0 {
        if let Err(code) = connection_check(connect) {
            return code;
        }
        return code(f(&*api(connect)));
    }
    let mut first = None;
    let mut done = false;
    let mut each = |result: Result<(), Error>| match result {
        Ok(()) => done = true,
        Err(error) => {
            first.get_or_insert(error);
        }
    };
    each(f(&*LOCAL));
    for remote in backend::all_remotes() {
        each(f(&remote.client));
    }
    match (done, first) {
        (true, _) | (false, None) => AE_SUCCESS,
        (false, Some(error)) => error_code(error),
    }
}

/// `AdsBeginTransaction`: begins a transaction on a connection, or nests one
/// in the one it has; with 0, on every connection.
#[unsafe(no_mangle)]
pub extern "system" fn AdsBeginTransaction(connect: AdsHandle) -> Unsigned32 {
    let _call = Call::begin("AdsBeginTransaction");
    on(connect, |api| api.begin_transaction(connect))
}

/// `AdsCommitTransaction`: commits a connection's transaction, or ends a
/// level of it; with 0, of every connection that has one.
/// `AE_TRANS_OUT_OF_SEQUENCE` without one.
#[unsafe(no_mangle)]
pub extern "system" fn AdsCommitTransaction(connect: AdsHandle) -> Unsigned32 {
    let _call = Call::begin("AdsCommitTransaction");
    on(connect, |api| api.commit_transaction(connect))
}

/// `AdsRollbackTransaction`: rolls a connection's transaction back, every
/// level of it; with 0, of every connection that has one.
#[unsafe(no_mangle)]
pub extern "system" fn AdsRollbackTransaction(connect: AdsHandle) -> Unsigned32 {
    let _call = Call::begin("AdsRollbackTransaction");
    on(connect, |api| api.rollback_transaction(connect, None))
}

/// `AdsRollbackTransaction80`: rolls a connection's transaction back, or
/// to the savepoint `savepoint` if it is not null, which keeps the
/// transaction (an unknown one fails with 7129). No options are defined.
///
/// # Safety
///
/// `savepoint` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsRollbackTransaction80(
    connect: AdsHandle,
    savepoint: *mut Unsigned8,
    _options: Unsigned32,
) -> Unsigned32 {
    let _call = Call::begin("AdsRollbackTransaction80");
    // SAFETY: `savepoint` is null or NUL-terminated, as # Safety says.
    let name = unsafe { text(savepoint) };
    on(connect, |api| {
        api.rollback_transaction(connect, name.clone())
    })
}

/// `AdsCreateSavepoint`: sets a savepoint in a connection's transaction; one
/// of that name goes first. Outside a transaction it fails with
/// `AE_TRANS_OUT_OF_SEQUENCE` (without the detail the other calls add, as
/// in the original engine).
///
/// # Safety
///
/// `savepoint` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsCreateSavepoint(
    connect: AdsHandle,
    savepoint: *mut Unsigned8,
    _options: Unsigned32,
) -> Unsigned32 {
    let _call = Call::begin("AdsCreateSavepoint");
    if let Err(code) = connection_check(connect) {
        return code;
    }
    // SAFETY: `savepoint` is null or NUL-terminated, as # Safety says.
    let Some(name) = (unsafe { text(savepoint) }) else {
        return fail(AE_INVALID_POINTER);
    };
    match api(connect).create_savepoint(connect, name) {
        Ok(()) => AE_SUCCESS,
        Err(Error::NotInTransaction) => {
            let text = crate::last_error::error_text(AE_TRANS_OUT_OF_SEQUENCE).unwrap_or_default();
            fail_text(
                AE_TRANS_OUT_OF_SEQUENCE,
                format!("Error {AE_TRANS_OUT_OF_SEQUENCE}:  {text}"),
            )
        }
        Err(error) => error_code(error),
    }
}

/// `AdsInTransaction`: whether a connection is in a transaction; with 0,
/// whether any is.
///
/// # Safety
///
/// `in_transaction` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsInTransaction(
    connect: AdsHandle,
    in_transaction: *mut Unsigned16,
) -> Unsigned32 {
    let _call = Call::begin("AdsInTransaction");
    let active = match connect {
        0 => {
            let remotes = backend::all_remotes();
            let mut any = LOCAL.in_transaction(0).unwrap_or(false);
            for remote in remotes {
                any |= remote.client.in_transaction(0).unwrap_or(false);
            }
            Ok(any)
        }
        _ => match connection_check(connect) {
            Ok(()) => api(connect).in_transaction(connect).map_err(error_code),
            Err(code) => Err(code),
        },
    };
    match active {
        // SAFETY: `in_transaction` is null or writable, as # Safety says.
        Ok(active) => unsafe { answer(Ok(()), in_transaction, Unsigned16::from(active)) },
        Err(code) => code,
    }
}

/// `AdsGetTransactionCount`: a connection's transaction nesting level, 0
/// outside one. Connection 0 is not one (`AE_INVALID_CONNECTION_HANDLE`), as
/// in the original engine.
///
/// # Safety
///
/// `count` must be null or valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsGetTransactionCount(
    connect: AdsHandle,
    count: *mut Unsigned32,
) -> Unsigned32 {
    let _call = Call::begin("AdsGetTransactionCount");
    if let Err(code) = connection_check(connect) {
        return code;
    }
    match api(connect).transaction_count(connect) {
        // SAFETY: `count` is null or writable, as # Safety says.
        Ok(n) => unsafe { answer(Ok(()), count, n) },
        Err(error) => error_code(error),
    }
}

/// `AdsFailedTransactionRecovery`: recovers the transactions a crash
/// interrupted (see [`tflip_engine::txlog`]), on the local server and every
/// server connected to; with `server`, the path of a connection, which
/// must be one (else `AE_NO_DRIVE_CONNECTION`, as the original's local
/// server answers). A transaction that cannot be recovered yet (a table of
/// it is open elsewhere) fails with 7036.
///
/// # Safety
///
/// `server` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AdsFailedTransactionRecovery(server: *mut Unsigned8) -> Unsigned32 {
    let _call = Call::begin("AdsFailedTransactionRecovery");
    // SAFETY: `server` is null or NUL-terminated, as # Safety says.
    if let Some(server) = unsafe { text(server) } {
        let wanted = server.to_ascii_lowercase();
        let wanted = wanted.trim_end_matches(['\\', '/']);
        let connected = tflip_engine::handle::connections(&LOCAL)
            .into_iter()
            .any(|(_, path)| path.to_ascii_lowercase().starts_with(wanted));
        if !connected {
            return fail(AE_NO_DRIVE_CONNECTION);
        }
    }
    let mut result = tflip_engine::txlog::recover(&LOCAL).map(drop);
    for remote in backend::all_remotes() {
        let done = remote.client.recover_transactions().map(drop);
        if result.is_ok() {
            result = done;
        }
    }
    code(result)
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use super::*;
    use crate::fixtures::{self, remote_twins};
    use crate::{
        ADS_ADT, ADS_EXCLUSIVE, ADS_SHARED, AdsCloseTable, AdsCreateSQLStatement,
        AdsExecuteSQLDirect, AdsGetLastError, AdsOpenTable90, AdsPackTable, AdsReindex,
        AdsSetTableTransactionFree, AdsZapTable,
    };

    /// The last error's text.
    fn last_error() -> String {
        let (mut code, mut text, mut len) = (0, [0u8; 600], 600u16);
        unsafe { AdsGetLastError(&mut code, text.as_mut_ptr(), &mut len) };
        String::from_utf8_lossy(&text[..usize::from(len)]).into_owned()
    }

    fn count(connect: AdsHandle) -> Unsigned32 {
        let mut n = 99;
        assert_eq!(
            unsafe { AdsGetTransactionCount(connect, &mut n) },
            AE_SUCCESS
        );
        n
    }

    fn active(connect: AdsHandle) -> Unsigned16 {
        let mut flag = 99;
        assert_eq!(unsafe { AdsInTransaction(connect, &mut flag) }, AE_SUCCESS);
        flag
    }

    fn name(text: &CStr) -> *mut Unsigned8 {
        text.as_ptr().cast_mut().cast()
    }

    /// The calls, their nesting and their errors, as the original's remote
    /// server answers them (`tools/ace/transactions/tx.c`).
    #[test]
    fn begin_commit_and_roll_back() {
        let _settings = fixtures::settings_shared();
        let connect = fixtures::connect("all_types");
        let null = std::ptr::null_mut();
        assert_eq!((count(connect), active(connect)), (0, 0));
        assert_eq!(AdsCommitTransaction(connect), AE_TRANS_OUT_OF_SEQUENCE);
        assert!(
            last_error().ends_with("valid sequence.  Not in a transaction."),
            "{}",
            last_error()
        );
        assert_eq!(AdsRollbackTransaction(connect), AE_TRANS_OUT_OF_SEQUENCE);
        let rc = unsafe { AdsCreateSavepoint(connect, name(c"sp1"), 0) };
        assert_eq!(rc, AE_TRANS_OUT_OF_SEQUENCE);
        assert!(
            last_error().ends_with("valid sequence."),
            "{}",
            last_error()
        );
        let mut n = 0;
        let rc = unsafe { AdsGetTransactionCount(0, &mut n) };
        assert_eq!(rc, crate::AE_INVALID_CONNECTION_HANDLE);

        assert_eq!(AdsBeginTransaction(connect), AE_SUCCESS);
        assert_eq!((count(connect), active(connect)), (1, 1));
        assert_eq!(AdsBeginTransaction(connect), AE_SUCCESS);
        assert_eq!(count(connect), 2);
        assert_eq!(AdsCommitTransaction(connect), AE_SUCCESS);
        assert_eq!(count(connect), 1);
        assert_eq!(AdsCommitTransaction(connect), AE_SUCCESS);
        assert_eq!((count(connect), active(connect)), (0, 0));

        assert_eq!(AdsBeginTransaction(connect), AE_SUCCESS);
        assert_eq!(
            unsafe { AdsCreateSavepoint(connect, name(c"sp1"), 0) },
            AE_SUCCESS
        );
        assert_eq!(
            unsafe { AdsCreateSavepoint(connect, null, 0) },
            crate::AE_INVALID_POINTER
        );
        let rc = unsafe { AdsRollbackTransaction80(connect, name(c"nosuch"), 0) };
        assert_eq!(rc, crate::AE_SAVEPOINT_NOT_FOUND);
        assert_eq!(
            unsafe { AdsRollbackTransaction80(connect, name(c"SP1"), 0) },
            AE_SUCCESS
        );
        assert_eq!(count(connect), 1);
        assert_eq!(
            unsafe { AdsRollbackTransaction80(connect, null, 0) },
            AE_SUCCESS
        );
        assert_eq!(count(connect), 0);
        assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
    }

    /// SQL's transaction statements share the connection's transaction.
    #[test]
    fn sql_statements() {
        let _settings = fixtures::settings_shared();
        let connect = fixtures::connect("all_types");
        let mut statement = 0;
        assert_eq!(
            unsafe { AdsCreateSQLStatement(connect, &mut statement) },
            AE_SUCCESS
        );
        let run = |sql: &CStr| unsafe {
            AdsExecuteSQLDirect(
                statement,
                sql.as_ptr().cast_mut().cast(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(run(c"COMMIT"), crate::SQL_ERROR);
        assert!(
            last_error().contains("NativeError = 5047;"),
            "{}",
            last_error()
        );
        assert_eq!(run(c"SAVEPOINT x"), crate::SQL_ERROR);
        assert_eq!(run(c"SET TRANSACTION AUTOCOMMIT_OFF"), AE_SUCCESS);
        assert_eq!(run(c"BEGIN TRANSACTION"), AE_SUCCESS);
        assert_eq!(run(c"BEGIN TRAN"), AE_SUCCESS);
        assert_eq!(count(connect), 2);
        assert_eq!(AdsCommitTransaction(connect), AE_SUCCESS);
        assert_eq!(run(c"SAVEPOINT a"), AE_SUCCESS);
        assert_eq!(run(c"ROLLBACK TO SAVEPOINT a"), AE_SUCCESS);
        assert_eq!(run(c"ROLLBACK TO nosuch"), crate::SQL_ERROR);
        assert!(
            last_error().ends_with("[ASA] The savepoint was not found."),
            "{}",
            last_error()
        );
        assert_eq!(run(c"SET TRANSACTION AUTOCOMMIT_ON"), crate::SQL_ERROR);
        assert!(
            last_error().contains("NativeError = 5063;"),
            "{}",
            last_error()
        );
        assert_eq!(run(c"COMMIT WORK"), AE_SUCCESS);
        assert_eq!(count(connect), 0);
        assert_eq!(run(c"BEGIN TRANSACTION"), AE_SUCCESS);
        assert_eq!(run(c"ROLLBACK WORK"), AE_SUCCESS);
        assert_eq!(count(connect), 0);
        assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
    }

    /// Packing, zapping, reindexing and making a table transaction-free are
    /// refused in a transaction, on an exclusive table (a shared one is
    /// refused for that first).
    #[test]
    fn illegal_commands() {
        let _settings = fixtures::settings_shared();
        let connect = fixtures::connect_to(&fixtures::scratch("illegal_commands", "all_types"));
        let open = |options| {
            let (mut table, null) = (0, std::ptr::null_mut());
            let file = name(c"all_types.adt");
            let rc = unsafe {
                AdsOpenTable90(
                    connect, file, null, ADS_ADT, 1, 1, 2, options, null, &mut table,
                )
            };
            assert_eq!(rc, AE_SUCCESS);
            table
        };
        assert_eq!(AdsBeginTransaction(connect), AE_SUCCESS);
        let shared = open(ADS_SHARED);
        assert_eq!(AdsPackTable(shared), crate::AE_TABLE_NOT_EXCLUSIVE);
        assert_eq!(AdsCloseTable(shared), AE_SUCCESS);
        let table = open(ADS_EXCLUSIVE);
        let illegal = crate::AE_ILLEGAL_COMMAND_DURING_TRANS;
        assert_eq!(AdsReindex(table), illegal);
        assert!(
            last_error().ends_with("transaction.  Cannot Reindex."),
            "{}",
            last_error()
        );
        assert_eq!(AdsPackTable(table), illegal);
        assert!(
            last_error().ends_with("Can't Pack in a transaction."),
            "{}",
            last_error()
        );
        assert_eq!(AdsZapTable(table), illegal);
        assert!(
            last_error().ends_with("Can't Zap in a transaction."),
            "{}",
            last_error()
        );
        assert_eq!(AdsSetTableTransactionFree(table, 1), illegal);
        assert_eq!(AdsRollbackTransaction(connect), AE_SUCCESS);
        assert_eq!(AdsReindex(table), AE_SUCCESS);
        assert_eq!(AdsCloseTable(table), AE_SUCCESS);
        assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
    }

    /// A table closed in a transaction: the records the transaction wrote
    /// stay locked, and the file opened, until it ends, as the help says
    /// the server holds them (`devguide_automatic_transactions`).
    #[test]
    fn closed_in_transaction() {
        let _settings = fixtures::settings_shared();
        use crate::{AdsGotoRecord, AdsLockRecord, AdsSetField, AdsWriteRecord};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/write");
        let (dir, _) = fixtures::scratch_from("closed-in-tx", &root.join("tx_basic/before"));
        let (writer, other) = (fixtures::connect_to(&dir), fixtures::connect_to(&dir));
        let open = |connect, options| {
            let (mut table, null) = (0, std::ptr::null_mut());
            let rc = unsafe {
                AdsOpenTable90(
                    connect,
                    name(c"t.adt"),
                    null,
                    ADS_ADT,
                    1,
                    1,
                    2,
                    options,
                    null,
                    &mut table,
                )
            };
            (rc, table)
        };
        assert_eq!(AdsBeginTransaction(writer), AE_SUCCESS);
        let (rc, table) = open(writer, ADS_SHARED);
        assert_eq!(rc, AE_SUCCESS);
        let mut value = *b"uno";
        assert_eq!(AdsGotoRecord(table, 1), AE_SUCCESS);
        assert_eq!(
            unsafe { AdsSetField(table, name(c"name"), value.as_mut_ptr(), 3) },
            AE_SUCCESS
        );
        assert_eq!(AdsWriteRecord(table), AE_SUCCESS);
        assert_eq!(AdsCloseTable(table), AE_SUCCESS);
        assert_eq!(open(other, ADS_EXCLUSIVE).0, crate::FILE_IN_USE);
        let (rc, seen) = open(other, ADS_SHARED);
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!(AdsLockRecord(seen, 1), crate::AE_LOCK_FAILED);
        assert_eq!(AdsCommitTransaction(writer), AE_SUCCESS);
        assert_eq!(AdsLockRecord(seen, 1), AE_SUCCESS);
        assert_eq!(AdsCloseTable(seen), AE_SUCCESS);
        let (rc, table) = open(other, ADS_EXCLUSIVE);
        assert_eq!(rc, AE_SUCCESS);
        assert_eq!(AdsCloseTable(table), AE_SUCCESS);
        for connect in [writer, other] {
            assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
        }
    }

    remote_twins!(
        begin_commit_and_roll_back,
        sql_statements,
        illegal_commands,
        closed_in_transaction,
        concurrent_transactions
    );

    /// Threads, each on a connection of its own, run transactions on one
    /// table at once: appends, changes and deletes, committed or rolled
    /// back, some refused by another's lock. Nothing deadlocks, and what
    /// was committed is there, in the table and its order.
    #[test]
    fn concurrent_transactions() {
        let _settings = fixtures::settings_shared();
        use crate::{
            ADS_IGNOREFILTERS, AdsAppendRecord, AdsAtEOF, AdsGetIndexHandle, AdsGetRecordCount,
            AdsGotoRecord, AdsGotoTop, AdsSetField, AdsSkip, AdsWriteRecord,
        };
        const THREADS: u32 = 6;
        const ROUNDS: u32 = 40;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/write");
        let (source, _) = fixtures::scratch_from("concurrent-tx", &root.join("tx_basic/before"));
        // A watchdog: a deadlock fails the test rather than hanging it.
        let (done, waiting) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            if waiting
                .recv_timeout(std::time::Duration::from_secs(120))
                .is_err()
            {
                eprintln!("concurrent_transactions: deadlocked");
                std::process::abort();
            }
        });
        let set = |table: AdsHandle, field: &CStr, value: &str| {
            let mut value = value.as_bytes().to_vec();
            let len = value.len() as u32;
            unsafe { AdsSetField(table, name(field), value.as_mut_ptr(), len) }
        };
        // The workers' threads connect as this one does (through the test
        // server when it runs remotely).
        let remote = fixtures::is_remote();
        let workers: Vec<_> = (0..THREADS)
            .map(|worker| {
                let source = source.clone();
                std::thread::spawn(move || {
                    let connect = match remote {
                        true => {
                            let mut connect = 0;
                            fixtures::remotely_with(|| connect = fixtures::connect_to(&source));
                            connect
                        }
                        false => fixtures::connect_to(&source),
                    };
                    let (mut table, null) = (0, std::ptr::null_mut());
                    let rc = unsafe {
                        AdsOpenTable90(
                            connect,
                            name(c"t.adt"),
                            null,
                            ADS_ADT,
                            1,
                            1,
                            2,
                            ADS_SHARED,
                            null,
                            &mut table,
                        )
                    };
                    assert_eq!(rc, AE_SUCCESS);
                    let mut committed = Vec::new();
                    for round in 0..ROUNDS {
                        let id = 1000 * (worker + 1) + round;
                        assert_eq!(AdsBeginTransaction(connect), AE_SUCCESS);
                        let mut ok = AdsAppendRecord(table) == AE_SUCCESS;
                        for rc in [
                            set(table, c"id", &id.to_string()),
                            set(table, c"name", &format!("w{worker}r{round}")),
                            set(table, c"m", &format!("memo {id}")),
                            AdsWriteRecord(table),
                        ] {
                            ok &= rc == AE_SUCCESS;
                        }
                        // A shared record, which another may hold: refused then.
                        if AdsGotoRecord(table, 1 + round % 3) == AE_SUCCESS
                            && set(table, c"m", &format!("touched by {id}")) == AE_SUCCESS
                        {
                            let _ = AdsWriteRecord(table);
                        }
                        let _ = crate::AdsCancelUpdate(table);
                        if ok && round % 3 != 0 {
                            assert_eq!(AdsCommitTransaction(connect), AE_SUCCESS);
                            committed.push(id);
                        } else {
                            assert_eq!(AdsRollbackTransaction(connect), AE_SUCCESS);
                        }
                    }
                    assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
                    committed
                })
            })
            .collect();
        let mut committed: Vec<u32> = workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect();
        let _ = done.send(());
        committed.sort_unstable();
        // What the table holds: the three records it had, and what was
        // committed, in record order and in its order.
        let connect = fixtures::connect_to(&source);
        let (mut table, null) = (0, std::ptr::null_mut());
        let rc = unsafe {
            AdsOpenTable90(
                connect,
                name(c"t.adt"),
                null,
                ADS_ADT,
                1,
                1,
                2,
                ADS_SHARED,
                null,
                &mut table,
            )
        };
        assert_eq!(rc, AE_SUCCESS);
        let mut count = 0;
        assert_eq!(
            unsafe { AdsGetRecordCount(table, ADS_IGNOREFILTERS, &mut count) },
            AE_SUCCESS
        );
        let walk = |handle: AdsHandle| -> Vec<u32> {
            let mut ids = Vec::new();
            assert_eq!(AdsGotoTop(handle), AE_SUCCESS);
            loop {
                let mut eof = 0;
                unsafe { AdsAtEOF(table, &mut eof) };
                if eof != 0 {
                    break;
                }
                let (mut buf, mut len) = ([0u8; 16], 16u32);
                let rc = unsafe {
                    crate::AdsGetField(table, name(c"id"), buf.as_mut_ptr(), &mut len, 0)
                };
                assert_eq!(rc, AE_SUCCESS);
                let text = std::str::from_utf8(&buf[..len as usize]).unwrap().trim();
                ids.push(text.parse::<u32>().unwrap());
                assert_eq!(AdsSkip(handle, 1), AE_SUCCESS);
            }
            ids.retain(|&id| id >= 1000);
            ids.sort_unstable();
            ids
        };
        assert_eq!(walk(table), committed);
        assert_eq!(count as usize, 3 + committed.len());
        let mut order = 0;
        assert_eq!(
            unsafe { AdsGetIndexHandle(table, name(c"name"), &mut order) },
            AE_SUCCESS
        );
        assert_eq!(walk(order), committed);
        assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
    }

    /// Crashes: a process (this test binary, running [`crash_child`]) writes
    /// in a transaction and ends without finishing it.
    mod crash {
        use std::path::{Path, PathBuf};
        use std::process::Command;

        use super::*;
        use crate::{
            ADS_CDX, ADS_IGNOREFILTERS, AdsAppendRecord, AdsAtEOF, AdsDeleteRecord,
            AdsGetIndexHandle, AdsGetRecordNum, AdsGotoRecord, AdsGotoTop, AdsSetField, AdsSkip,
            AdsWriteRecord,
        };

        /// The tables: `t` (ADT, its order `name`), `cdx_t` (FoxPro, its
        /// order `cdx_name`), as `fixtures/write/tx_basic` and `tx_dbf` set
        /// them up.
        fn tables(test: &str) -> PathBuf {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/write");
            let (_, dir) = fixtures::scratch_from(test, &root.join("tx_basic/before"));
            for file in ["cdx_t.dbf", "cdx_t.cdx", "cdx_t.fpt"] {
                std::fs::copy(root.join("tx_dbf/before").join(file), dir.join(file)).unwrap();
            }
            dir
        }

        fn open(connect: AdsHandle, table: &CStr, table_type: Unsigned16) -> AdsHandle {
            let (mut handle, null) = (0, std::ptr::null_mut());
            let rc = unsafe {
                AdsOpenTable90(
                    connect,
                    name(table),
                    null,
                    table_type,
                    1,
                    1,
                    2,
                    ADS_SHARED,
                    null,
                    &mut handle,
                )
            };
            assert_eq!(rc, AE_SUCCESS, "{}", last_error());
            handle
        }

        fn set(table: AdsHandle, field: &CStr, value: &str) {
            let mut value = value.as_bytes().to_vec();
            let len = value.len() as u32;
            let rc = unsafe { AdsSetField(table, name(field), value.as_mut_ptr(), len) };
            assert_eq!(rc, AE_SUCCESS, "{}", last_error());
        }

        fn change(table: AdsHandle, record: u32, values: &[(&CStr, &str)]) {
            assert_eq!(AdsGotoRecord(table, record), AE_SUCCESS);
            values
                .iter()
                .for_each(|(field, value)| set(table, field, value));
            assert_eq!(AdsWriteRecord(table), AE_SUCCESS, "{}", last_error());
        }

        fn append(table: AdsHandle, values: &[(&CStr, &str)]) {
            assert_eq!(AdsAppendRecord(table), AE_SUCCESS);
            values
                .iter()
                .for_each(|(field, value)| set(table, field, value));
            assert_eq!(AdsWriteRecord(table), AE_SUCCESS, "{}", last_error());
        }

        fn delete(table: AdsHandle, record: u32) {
            assert_eq!(AdsGotoRecord(table, record), AE_SUCCESS);
            assert_eq!(AdsDeleteRecord(table), AE_SUCCESS);
        }

        /// The transaction the processes run, in the folder
        /// `TFLIP_TEST_CRASH_DIR`, ended as `TFLIP_TEST_CRASH_END` says:
        /// `commit`, `rollback`, or not at all (the process ends); with
        /// `wait`, committed once the folder has a file `go` (after the
        /// process made one named `ready`); or with `open`, the tables only
        /// opened and closed.
        #[test]
        #[ignore = "run by the crash tests in a process of its own"]
        fn crash_child() {
            let Some(dir) = std::env::var_os("TFLIP_TEST_CRASH_DIR") else {
                return;
            };
            let connect = fixtures::connect_to(&PathBuf::from(dir).display().to_string());
            let t = open(connect, c"t.adt", ADS_ADT);
            let cdx = open(connect, c"cdx_t.dbf", ADS_CDX);
            if std::env::var("TFLIP_TEST_CRASH_END").as_deref() == Ok("open") {
                assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
                return;
            }
            assert_eq!(AdsBeginTransaction(connect), AE_SUCCESS);
            change(t, 1, &[(c"name", "uno"), (c"m", "memo uno")]);
            assert_eq!(
                unsafe { AdsCreateSavepoint(connect, name(c"s"), 0) },
                AE_SUCCESS
            );
            change(t, 3, &[(c"name", "tres"), (c"m", "memo tres")]);
            append(
                t,
                &[(c"id", "11"), (c"name", "eleven"), (c"m", "memo eleven")],
            );
            let rc = unsafe { AdsRollbackTransaction80(connect, name(c"s"), 0) };
            assert_eq!(rc, AE_SUCCESS);
            append(t, &[(c"id", "10"), (c"name", "ten"), (c"m", "memo ten")]);
            delete(t, 2);
            change(t, 3, &[(c"name", "drei")]);
            change(cdx, 1, &[(c"name", "uno"), (c"m", "memo uno")]);
            append(cdx, &[(c"id", "10"), (c"name", "ten"), (c"m", "memo ten")]);
            delete(cdx, 2);
            match std::env::var("TFLIP_TEST_CRASH_END").as_deref() {
                Ok("wait") => {
                    let dir = PathBuf::from(std::env::var_os("TFLIP_TEST_CRASH_DIR").unwrap());
                    std::fs::write(dir.join("ready"), b"").unwrap();
                    let start = std::time::Instant::now();
                    while !dir.join("go").exists() {
                        assert!(start.elapsed().as_secs() < 120, "never told to go on");
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    assert_eq!(AdsCommitTransaction(connect), AE_SUCCESS);
                }
                Ok("commit") => assert_eq!(AdsCommitTransaction(connect), AE_SUCCESS),
                Ok("rollback") => assert_eq!(AdsRollbackTransaction(connect), AE_SUCCESS),
                _ => std::process::exit(3),
            }
            assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
        }

        /// Runs [`crash_child`] on `dir`, logging to `log`, with `envs`:
        /// its exit code.
        fn child(dir: &Path, log: &Path, envs: &[(&str, &str)]) -> Option<i32> {
            child_command(dir, log, envs).status().unwrap().code()
        }

        fn child_command(dir: &Path, log: &Path, envs: &[(&str, &str)]) -> Command {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "transaction::tests::crash::crash_child",
                    "--ignored",
                ])
                .args(["--test-threads", "1", "--quiet"])
                .env("TFLIP_TEST_CRASH_DIR", dir)
                .env("TFLIP_TX_LOG", log)
                .envs(envs.iter().copied())
                .stdout(std::process::Stdio::null());
            command
        }

        /// The value of `field` in record `record` of `table` (in `dir`),
        /// and whether this process can lock the record.
        fn read(dir: &Path, table: &CStr, table_type: Unsigned16, record: u32) -> (String, bool) {
            let connect = fixtures::connect_to(&dir.display().to_string());
            let table = open(connect, table, table_type);
            assert_eq!(AdsGotoRecord(table, record), AE_SUCCESS);
            let (mut buf, mut len) = ([0u8; 64], 64u32);
            let rc =
                unsafe { crate::AdsGetField(table, name(c"name"), buf.as_mut_ptr(), &mut len, 0) };
            assert_eq!(rc, AE_SUCCESS);
            let value = String::from_utf8_lossy(&buf[..len as usize])
                .trim_end()
                .to_owned();
            let locked = crate::AdsLockRecord(table, record) == AE_SUCCESS;
            assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
            (value, locked)
        }

        /// Another process's transaction, running: this process reads the
        /// tables as they were (each record once in its order, at its old
        /// key; neither the appended records nor the new values), cannot
        /// lock what it wrote, and leaves its log alone; once it commits,
        /// sees what it committed.
        #[test]
        fn other_process() {
            let log = std::env::temp_dir().join("tflip-crash-log-other-process");
            let _ = std::fs::remove_dir_all(&log);
            let running = tables("other-process");
            let before = tables("other-process-before");
            let committed = tables("other-process-committed");
            let other = log.with_extension("committed");
            let envs = [("TFLIP_TEST_CRASH_END", "commit")];
            assert_eq!(child(&committed, &other, &envs), Some(0));
            let _ = std::fs::remove_dir_all(&other);
            let envs = [("TFLIP_TEST_CRASH_END", "wait")];
            let mut process = child_command(&running, &log, &envs).spawn().unwrap();
            let start = std::time::Instant::now();
            while !running.join("ready").exists() {
                assert!(
                    start.elapsed().as_secs() < 120,
                    "the other process never got ready"
                );
                assert!(
                    process.try_wait().unwrap().is_none(),
                    "the other process ended"
                );
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let orders = [
                (c"t.adt", ADS_ADT, c"name"),
                (c"cdx_t.dbf", ADS_CDX, c"cdx_name"),
            ];
            for (table, table_type, order) in orders {
                assert_eq!(
                    walk(&running, table, table_type, order),
                    walk(&before, table, table_type, order)
                );
                assert_eq!(
                    read(&running, table, table_type, 1),
                    ("one".to_owned(), false)
                );
            }
            assert_eq!(
                read(&running, c"t.adt", ADS_ADT, 3),
                ("three".to_owned(), false)
            );
            assert_eq!(tflip_engine::txlog::recover_in(&crate::LOCAL, &log), Ok(0));
            std::fs::write(running.join("go"), b"").unwrap();
            assert_eq!(process.wait().unwrap().code(), Some(0));
            for (table, table_type, order) in orders {
                assert_eq!(
                    walk(&running, table, table_type, order),
                    walk(&committed, table, table_type, order)
                );
                assert_eq!(
                    read(&running, table, table_type, 1),
                    ("uno".to_owned(), true)
                );
            }
            assert_eq!(
                read(&running, c"t.adt", ADS_ADT, 3),
                ("drei".to_owned(), true)
            );
            let _ = std::fs::remove_dir_all(&log);
        }

        /// The records of `table` (in `dir`) in the order `order`.
        fn walk(dir: &Path, table: &CStr, table_type: Unsigned16, order: &CStr) -> Vec<u32> {
            let connect = fixtures::connect_to(&dir.display().to_string());
            let table = open(connect, table, table_type);
            let mut index = 0;
            assert_eq!(
                unsafe { AdsGetIndexHandle(table, name(order), &mut index) },
                AE_SUCCESS
            );
            assert_eq!(AdsGotoTop(index), AE_SUCCESS);
            let mut records = Vec::new();
            loop {
                let mut eof = 0;
                assert_eq!(unsafe { AdsAtEOF(table, &mut eof) }, AE_SUCCESS);
                if eof != 0 {
                    break;
                }
                let mut record = 0;
                let rc = unsafe { AdsGetRecordNum(table, ADS_IGNOREFILTERS, &mut record) };
                assert_eq!(rc, AE_SUCCESS);
                records.push(record);
                assert_eq!(AdsSkip(index, 1), AE_SUCCESS);
            }
            assert_eq!(crate::AdsDisconnect(connect), AE_SUCCESS);
            records
        }

        /// The process crashed as `envs` make it; recovered (by
        /// `AdsFailedTransactionRecovery`, or with `on_open` by the next
        /// process to open a table), the tables are as when it ends the
        /// transaction with `end` (record for record, memo for memo, and
        /// their orders key for key).
        fn crashed_like(test: &str, envs: &[(&str, &str)], end: &str, on_open: bool) {
            let log = std::env::temp_dir().join(format!("tflip-crash-log-{test}"));
            let _ = std::fs::remove_dir_all(&log);
            let crashed = tables(&format!("{test}-crashed"));
            let ended = tables(&format!("{test}-{end}"));
            assert_eq!(child(&crashed, &log, envs), Some(3));
            assert_eq!(std::fs::read_dir(&log).unwrap().count(), 1);
            let other = log.with_extension("ended");
            assert_eq!(
                child(&ended, &other, &[("TFLIP_TEST_CRASH_END", end)]),
                Some(0)
            );
            let _ = std::fs::remove_dir_all(&other);
            match on_open {
                true => {
                    let envs = [("TFLIP_TEST_CRASH_END", "open")];
                    assert_eq!(child(&crashed, &log, &envs), Some(0));
                }
                false => {
                    assert_eq!(tflip_engine::txlog::recover_in(&crate::LOCAL, &log), Ok(1));
                }
            }
            assert_eq!(std::fs::read_dir(&log).unwrap().count(), 0);
            for file in ["t.adt", "t.adm", "cdx_t.dbf", "cdx_t.fpt"] {
                let read = |dir: &Path| std::fs::read(dir.join(file)).unwrap();
                assert!(read(&crashed) == read(&ended), "{file} differs");
            }
            assert_eq!(
                walk(&crashed, c"t.adt", ADS_ADT, c"name"),
                walk(&ended, c"t.adt", ADS_ADT, c"name")
            );
            assert_eq!(
                walk(&crashed, c"cdx_t.dbf", ADS_CDX, c"cdx_name"),
                walk(&ended, c"cdx_t.dbf", ADS_CDX, c"cdx_name")
            );
            let _ = std::fs::remove_dir_all(&log);
        }

        /// A crash while the transaction writes: it is rolled back.
        #[test]
        fn in_transaction() {
            crashed_like("crash-in-transaction", &[], "rollback", false);
        }

        /// The same, recovered as the next process opens a table.
        #[test]
        fn in_transaction_then_open() {
            crashed_like("crash-then-open", &[], "rollback", true);
        }

        /// A crash while the transaction commits (after its first record):
        /// the commit is finished. The engine has the crash point in debug
        /// builds only.
        #[test]
        #[cfg(debug_assertions)]
        fn in_commit() {
            let envs = [
                ("TFLIP_TEST_CRASH_END", "commit"),
                ("TFLIP_TEST_CRASH_IN_COMMIT", "1"),
            ];
            crashed_like("crash-in-commit", &envs, "commit", false);
        }
    }
}
