# Transactions

Notes for the design of transactions in tflip: what the original engine
does, as probed and as its help describes it, and the questions that
follow, and the plan tflip followed (see [Plan](#plan)); where tflip
differs is in [KNOWN_ISSUES.md](KNOWN_ISSUES.md#transactions).

## The local server, probed

Probed with [`tools/ace/transactions/tx.c`](../tools/ace/transactions/tx.c)
on the original's local server (12.0.0.2), the only engine the probes can
reach. The help says it "ignores transaction-related instructions"
(`devguide_using_transactions`), and it does, but for a nesting count:

- **Nothing is held back.** A record written in a transaction is in the
  table and index files at once, another connection reads the new value,
  a rollback undoes nothing, and an append stays. No log file appears in
  the data folder. Writes take no locks that outlast them: another
  connection locks a record changed in the transaction, and
  `AdsUnlockRecord` on it fails with 5062 (no lock).
- **Nothing is illegal**, but for a table lock. `AdsPackTable`,
  `AdsZapTable`, `AdsReindex`, `AdsSetTableTransactionFree` and
  `AdsCloseTable` succeed in a transaction (where the table is open as they
  need), and so does `AdsDisconnect`. A table the handle has locked with
  `AdsLockTable`, before the transaction or in it, cannot be unlocked
  (`AdsUnlockTable` fails with 5063 and the lock stays until the transaction
  ends) and `AdsCloseTable` of it fails with 5063 but closes it all the same
  (`fixtures/errors/locks_in_transaction.json`). Record locks are let go
  of as usual.
- **A nesting count per connection**, as `AdsGetTransactionCount` reports
  (the help says it is always 0 on the local server; it is not):
  - `AdsBeginTransaction(c)` adds 1, `AdsCommitTransaction(c)` takes 1
    away, `AdsRollbackTransaction(c)` and `AdsRollbackTransaction80(c,
    NULL)` set it to 0;
  - a commit or rollback at 0 fails with 5047 `AE_TRANS_OUT_OF_SEQUENCE`,
    "… Not in a transaction.", also with handle 0 when no connection is in
    one;
  - `AdsBeginTransaction(0)` adds to every connection (one connection went
    from 0 to 2, the other to 1: not worked out), and
    `AdsRollbackTransaction(0)` sets them all to 0.
- **Savepoints**: `AdsCreateSavepoint` outside a transaction fails with
  5047 (without "Not in a transaction."), with a NULL name with 5021
  (Parameter 2); in one it succeeds. `AdsRollbackTransaction80` to any
  name, an unknown one too, succeeds and keeps the count.
- **`AdsInTransaction`**: 1 while the connection has a count (with handle 0,
  while any has); the SQL statements leave it 0, as they leave the count.
  An unknown handle fails with 5024 (the last error 5018), and so does
  `AdsGetTransactionCount(0)`. (The first probe read 0 here: it wrote the
  count through a pointer to a 16-bit variable next to the flag.)
- **SQL**: `BEGIN TRAN[SACTION]`, `COMMIT [WORK]`, `ROLLBACK [WORK]`,
  `ROLLBACK TO [SAVEPOINT] name`, `SAVEPOINT name`, `SET TRANSACTION
  AUTOCOMMIT_ON` and `sp_IgnoreTableTransactions` succeed in any order,
  without a transaction too, and leave the API's count alone; an `UPDATE`
  between them stays. (The help says the local server checks their
  sequence; it does not.)
- **`AdsFailedTransactionRecovery(NULL)`** succeeds; with the data folder
  it fails with 5033 (no connected server for the drive).

Also seen: an SQL `UPDATE` of a record another connection had locked
returned 0 and changed nothing. To probe on its own.

An external procedure's library on the local server
(`fixtures/sql/aeprules.json`): its `AdsBeginTransaction`,
`AdsCommitTransaction` and `AdsRollbackTransaction` on the handle it is
given change the caller's count (a begin, then the caller's
`AdsGetTransactionCount` is 1; a commit takes it back; an unbalanced
rollback is 5047); its SQL `BEGIN`, `COMMIT` and `ROLLBACK` leave the count
alone. A library's `AdsConnect60` of server type 7 connected, under Wine, to
the evaluation server running beside it: probes use type 1.

So tflip can match the local server, files and all, by keeping the count
and checking the sequence, and writing as it writes today.

## The remote server, probed

Probed with the same program on the ADS 12 evaluation server, which behaves
as the full server but for its license count, run under Wine (setup in `LOCAL.md`) and reached over TCP
(`CommType=TCP_IP`; tables named by their full server path, as the
original takes relative names from the client's folder):

- **Changes wait in the log until the commit**, as the help says; byte by
  byte (`fixtures/write/tx_basic`, recorded with `tools/ace/write/run.sh` on
  the server, with snapshots of the files during the script):
  - a changed or deleted ADT record keeps its old data in the file; only
    its first two bytes change, `04 00` to `84 04`, which marks the record
    as in a transaction (the help's "TPS semaphore byte", error 7042). A
    rollback sets them back; a commit writes the new data, unmarked;
  - an appended record is written blank with `11 04`, the table's record
    count one higher; a rollback makes it a deleted record on the free
    list (`05 00`, the header's free-list fields);
  - memo values are written to the `.adm` at once, and their blocks freed
    again on a rollback (the data stays);
  - the `.adi` header changes at the first write (bytes 0x0E and 0x19) and
    keeps that after the rollback; keys change at the commit.
  The log, `_t-#####.TPS` (a hex number) in the server's `C:\`, is created
  by `AdsBeginTransaction` (31 bytes), grows with each write (3,087 bytes
  after one change, 5,522 after an append) and is emptied at the end; two
  appear per transaction.
- **The mark's second byte** varies from run to run (`84 04`, `84 06`,
  `84 07`): the server's own number (`snaps-volatile.json`).
- **DBF tables** (`fixtures/write/tx_dbf`, FoxPro, VFP and Clipper alike):
  a changed or deleted record's deletion byte gets bit 7 (`20` to `A0`),
  its data staying; an appended record is written at the end with the
  deletion byte `0`, blank fields but its memo reference (the memo value
  written at once), the header's count one higher; a changed record's memo
  value waits for the commit. A rollback clears the marks and makes the
  appended record `*` deleted with a blank memo reference; an `.fpt`'s
  next free block goes back, a `.dbt` keeps its length. The commit writes
  the data, the deletions (`*`) and appends (` `).
- **Several tables** (`fixtures/write/tx_multi`): one transaction spans
  them. A unique order refuses another connection's key that the
  transaction took (7057), while that connection still reads the old
  record; its record count includes the blank appended record, and the
  order holds the old and the new keys (4 for 2 records) until the end.
- **A client that goes away** (`fixtures/write/tx_abandon`): the server
  rolls its transaction back as `AdsRollbackTransaction` does; once the
  files close, the `.adi` header bytes are back too, and the files equal
  the ones before but for the appended (now deleted) record and the memo
  value's bytes.
- **The server caches index pages**: while it runs, a snapshot of an
  `.adi` still has keys that changed long before (even after
  `AdsFlushFileBuffers`); only the files after the connection closes are
  complete. The table and memo files show each step.
- **Others see the data as it was**: another connection reads the record's
  old value while the transaction runs, and the new one after the commit.
- **Locks**: a record written in a transaction is locked by it
  (`AdsIsRecordLocked` 1 on its own handle); another connection's
  `AdsLockRecord` fails with 5035. `AdsUnlockRecord` on it succeeds, and the
  record stays locked until the transaction ends. Two handles of the same
  table on one connection lock separately: what one holds, or wrote in a
  transaction, the other cannot lock (5035), write or unlock (5062);
  `AdsIsRecordLocked` says 1 only on the first (`fixtures/write/tx_handles`).
- **Illegal in a transaction** (5063, with "Cannot Reindex.", "Can't Pack
  in a transaction.", "Can't Zap in a transaction."): `AdsReindex`,
  `AdsPackTable`, `AdsZapTable`, `AdsSetTableTransactionFree`, and SQL's
  `SET TRANSACTION`. Closing a table in a transaction succeeds, and so does
  `AdsDisconnect` (the transaction is rolled back); a table lock is as on
  the local server (5063 from `AdsUnlockTable` and from `AdsCloseTable`,
  which closes the table all the same).
- **The count**: as on the local server (`AdsBeginTransaction` adds 1,
  commit takes 1, rollback ends it; 5047 out of sequence), but
  `AdsBeginTransaction(0)` adds 1 to each connection. `AdsInTransaction` is
  1 in a transaction, the SQL statements' too; `SET TRANSACTION` succeeds
  outside one.
- **Savepoints**: an unknown one is 7129 ("The savepoint was not found."),
  `AdsCreateSavepoint` outside a transaction 5047; a rollback to one keeps
  the transaction and its count.
- **SQL shares the transaction**: `BEGIN TRANSACTION` adds to the API's
  count (twice is 2), `COMMIT` ends what the API began, a `ROLLBACK TO
  SAVEPOINT` undoes an `UPDATE`, and `COMMIT`, `ROLLBACK` and `SAVEPOINT`
  outside a transaction fail with 7200 (native 5047).
  `sp_IgnoreTableTransactions` on a table another connection has open fails
  with 7008.
- **`AdsFailedTransactionRecovery(NULL)`** succeeds.
- **Changes pending in a handle** when the transaction ends (set, deleted,
  appended, not written; `fixtures/write/tx_pending`): the commit writes
  them as part of it (another connection reads them at once, unlocked);
  a rollback takes them back (the handle reads the record as it was, a
  pending delete is gone, a pending append is a deleted record on the free
  list); nothing is written when the handle moves on.

**Free tables and dictionaries alike**: the same probe on a new data
dictionary (connected to as `ADSSYS`, tables opened by their names) gives
the same answers, errors, locks, log files and file changes as on free
tables; only two error texts differ in their details (7008 names no file,
5063 has its `Error 5063:` prefix).
What only a dictionary has was not probed yet: a table's
transaction-free property (`ADS_DD_TABLE_TRANS_FREE`), triggers (which the
help says run in a transaction of their own), and referential integrity.

- **Appends** (`fixtures/write/tx_append`): `AdsAppendRecord` in a
  transaction writes the blank marked record at once (ADT `11 xx`, DBF
  `0`); a cancelled append stays so until the transaction ends, and the
  commit deletes it (ADT onto the free list with NULL values, DBF `*`).
  `AdsFlushFileBuffers` leaves the pending record alone (the help: it has
  no effect on a server).
- **Memo values of DBF records** (`tx_dbf_memo`, `tx_vfp`): a record
  without a memo block gets the value at once, its reference in the marked
  record, and a rollback gives the block back; one with a block waits for
  the commit, which writes it as a write does (in place if it fits).
- **Memo values of ADT records on a table open exclusively**
  (`tx_memo_exclusive`, `tx_memo_sizes`): the rule of a DBF table's
  applies, but by what `store_memo` would do. A value that fits the blocks
  of the one it replaces, or clears it, waits for the commit (nothing in the
  `.adm`, the record's reference unchanged; the commit writes it in place).
  One that needs a new block (a record with no value, an append, a value of
  more or fewer blocks) is written at once, and only its block number goes
  into the marked record, its length staying until the commit; a rollback
  gives the blocks back and keeps their data, and puts the numbers back. A
  shared table's values are all written at once, to blocks of their own.
- **Savepoints and appended records** (`tx_savepoint_lock`,
  `tx_savepoint_memo`): a rollback to a savepoint takes the writes back
  newest first, so appended slots go onto the free list in the opposite
  order to a whole rollback's. It lets go of the locks of the appended
  records it takes back, and the commit's release of them then fails with
  7138 after it committed (the transaction is over, its count 0); not for
  a table open exclusively, for DBF tables, for records SQL appended,
  after a whole rollback, or when an append took the slot again. A record deleted in a
  transaction stays locked by it, so the handle that a failed
  `AdsGotoRecord` left on it may delete or change it again (a table open
  shared outside a transaction refuses with 5035), and the commit keeps the
  values it set (`tx_redelete`).
- **Handles after a rollback** (`tx_cursor_after`): a handle on a record
  that a rollback to a savepoint freed keeps the record's image and (ADT)
  its record count; a whole rollback leaves an ADT handle on a freed slot
  at BOF, its record number the slot count + 1, `skip 1` going to record 1
  and `skip -1` staying at BOF.
- **Full-text orders on FoxPro and VFP tables** (`fts_tx_cdx_*`,
  `fts_tx_vfp_*`, the files only): as an ADT table's, the orders hold no
  word of a transaction until its commit, which takes a record's words
  once, from its original to its final image (memo values that waited are
  written before the new words are read, the original's before). A
  rollback gives a DBF table's memo blocks back newest first (both of two
  appended records' go, an `.fpt`'s next free block back at the first
  one's). Not a part of this, but met meanwhile: a DBF memo value written
  in place (the commit's, or a write's outside a transaction) goes to a new
  block at the end of the `.fpt` when another connection's handle stands on
  the record.
- **SQL statements** (`tx_sql`, each case on a table of its own, on the
  remote and the local server): a statement is not atomic. One that fails
  keeps what it wrote before (the failed `INSERT`'s record deleted); an
  `UPDATE` or `DELETE` passes by records another connection has locked and
  fails with 7200 (5035) at the end. `SET TRANSACTION AUTOCOMMIT_ON` puts
  each `INSERT`, `UPDATE` and `DELETE` outside a transaction in one of its
  own, committed after it, rolled back if it fails (a lock too);
  `AUTOCOMMIT_OFF` begins one the statement leaves open, which a failure
  rolls back (one that fails in a transaction already open keeps what it
  wrote); `AdsBeginTransaction` and `BEGIN TRANSACTION` set the mode back
  to `EXPLICIT`. A `SELECT` and the API's writes begin nothing. A rollback
  frees appended records in the order the transaction wrote them. The
  local server ignores the modes. (Reads of a record a handle is already
  on are stale in the original: it does not read it again.)
- **A dictionary's transaction-free tables**: `sp_ModifyTableProperty`'s
  `TABLE_TRANS_FREE` sets the `.adt` header's flag too
  (`tools/ace/dd/variants.txt`), which tflip honours.

- **Triggers** (`tools/ace/sql/triggers.txt`, `fixtures/sql/triggers.json`,
  recorded on the remote server). Each record a statement changes runs with
  the triggers it fires in a transaction of its own, two levels deep
  (`::conn.TransactionCount` reads 2 in a trigger, 3 inside the client's
  transaction). A failing trigger (5177, its error inside) takes back that
  record's change and what the triggers wrote; the records the statement
  changed before stay, and the client's transaction goes on at its own level.
  `ROLLBACK` in such a trigger is 5047, even after a `BEGIN TRANSACTION` of
  its own (which, with `COMMIT`, works). A trigger made `NO TRANSACTION` runs
  at the client's level and keeps what it wrote when it fails, and so does a
  failing `AFTER UPDATE`/`AFTER DELETE` the change; but the record a failing
  `AFTER INSERT` fired on is gone either way. The local server runs every
  trigger as `NO TRANSACTION` (a count of 0), accepts `ROLLBACK` in one, and
  drops a failing `AFTER INSERT`'s record too. tflip does the remote server's
  with real transactions and the local server's without.

- **Scripts, procedures, triggers and referential integrity**
  (`fixtures/sql/txscripts.json`, `txprocs.json`, `txtrig.json`, recorded on
  the remote server with `tools/ace/sql/txscripts.txt`, `txprocs.txt` and
  `txtrig.txt`, replayed):
  - **A script** begins, ends and nests as the API's calls do (`BEGIN
    TRANSACTION; BEGIN TRANSACTION; COMMIT;` leaves a count of 1), and
    `COMMIT` or `ROLLBACK` with none begun is 5047, the writes before it
    staying. A script that leaves its transaction open leaves the count
    (`EXECUTE IMMEDIATE 'BEGIN TRANSACTION'` too). A script that fails takes
    back the transaction it began (the count is 0 after it) when the client had
    none; with the client's own transaction open, the failure leaves the count
    and the writes (the levels the script began stay too), and a failing
    `INSERT` there counts the record it appended. Around `TRY ... CATCH` a
    caught error takes nothing back: the transaction stays open, a `CATCH`
    that does not end it leaves the count, one that commits or rolls back ends
    it; an error nothing catches is a failing script's.
  - **A stored procedure is a call level**: it ends at its end what it left
    open (committed, when the caller had no transaction: the count is back at
    the caller's, `BEGIN TRANSACTION; INSERT ...;` without `COMMIT` leaves the
    row there), may `COMMIT` or `ROLLBACK` only levels it began (5047
    otherwise, inside it and even in the caller's transaction), may not roll
    back a transaction the caller began (5047), and rolls back to its own
    savepoints only (7129 for the caller's, and its own are gone after it).
    One that fails ends the same way, but rolls back the writes only if the
    caller had no transaction; the rows it wrote do not count for the
    procedure that called it. `::conn.TransactionCount` in it is the caller's
    plus its own. A function (UDF) is the same (a `COMMIT` in it is 5047).
  - **An external procedure's library** on the handle it is given
    (`fixtures/sql/aeprulesremote.json`, `tools/ace/aep/aeprulesremote.txt`)
    is a call level too, but for the API's calls only: `AdsBeginTransaction`
    nests; `AdsCommitTransaction` ends only a level the library began (5047
    otherwise); `AdsRollbackTransaction` only a transaction it began outside
    the caller's (5047 in the caller's, the level it began ending without
    undoing anything). A refused API call in the caller's transaction spoils
    it: its later commits and rollbacks happen, and are answered 7207 (`AQE`
    7200 through SQL) until it ends, and the library's `AdsInTransaction` sees
    only the levels it began. The SQL `COMMIT` and `ROLLBACK` follow the
    procedure's rules (7200 with 5047 inside); outside the caller's
    transaction, one that ends the level the library began with SQL's `BEGIN`
    does, and answers a bare 5047. The library's own `AdsConnect60` is a
    separate connection of server type 4 (`ADS_AIS_SERVER`) for the types 0,
    4, 5, 6 and 7, 6097 for those with a remote server and no 4 (2, 3), 6082
    for 1.
  - **A trigger** is a call level as well (inside the two levels its record's
    change runs in): `COMMIT` or `ROLLBACK` with nothing of its own begun is
    5047, the caller's savepoints are 7129, its own go with it, and what it
    began and did not end ends with it. `NO TRANSACTION` on one trigger of an
    event is for all of them: none runs in a transaction of its own, and all
    read the client's count. (7130 and 7187, which the texts promise for a
    rollback to a savepoint before the trigger's start and for an unended
    transaction in a sub program, were not seen in any of these; the caller's
    savepoint is not found, and an unended transaction ends with the call;
    tried again on the remote server with nested triggers, a procedure
    calling a procedure, scripts and a trigger calling a procedure: 7129,
    `fixtures/events/events_tx_sub.json`.)
  - **A cascading rule** (`ON DELETE`/`ON UPDATE` CASCADE, SET NULL) cannot
    lock a child the caller's transaction wrote (written by itself, or
    inserted): 5035 with the record number, `This record lock was necessary
    because of a cascaded update or delete due to exising referential
    integrity rules`, before it changes any child, and the same the other way
    (a statement cannot lock what a rule wrote in the transaction). RESTRICT
    refuses as outside a transaction. SQL statements of the same
    connection lock what the transaction wrote, whichever handle they use.
  - **The ADT free list** (`fixtures/write/tx_free`): a slot deleted in the
    transaction is not taken by an append in it (the commit frees it, and the
    next append takes it), a slot freed before it is, a rollback frees the
    appended slots in the order they were written. A record recalled in the
    transaction that deleted it still goes on the free list at the commit
    (`fixtures/write/tx_recall`, an ADS bug).
  - **DDL**: `CREATE INDEX`, `CREATE TABLE` and `DROP TABLE` run in a
    transaction and stay after a rollback; `ALTER TABLE` that restructures
    (online or not) is 5063 `Cannot restructure a table from within a
    transaction.`, `DROP INDEX` 5063 `Can't delete index orders.`
    (`fixtures/sql/txddl.json`, `txremote.json`).
  - **Statements**: a server refuses a connection's statement that runs the
    first time while 50 (`sp_SetStatementLimit`; 0 or less none) have run on it
    and are not closed: 7216 `SQL statement (query) limit exceeded.`, also for
    one that gives no cursor, and with the cursors closed; `AdsCloseSQLStatement`
    gives the place back (`fixtures/sql/txlimit.json`, `tools/ace/sql/txlimit.txt`).
  - **A connection's remote facts** (`fixtures/sql/txremote.json`):
    `::conn.ClientHostName` is the client's machine name, `NetworkAddress` the
    address the server sees, `IsRoot` false; `sp_GetSecurityInfo` says
    `TCP_IP` for the communication type; `CREATE ONLINE INDEX` and `ALTER ONLINE
    TABLE` run (also `ONLINE_WITH_TRANSITION`) and give the results and errors
    the offline statements do, the forms `CREATE UNIQUE ONLINE INDEX`, `ALTER
    TABLE ... ONLINE` and `DROP ONLINE INDEX` being syntax errors.
  - **Online packs and reindexes wait for transactions**
    (`fixtures/events/events_online_procs.json`): `sp_PackTableOnline`,
    `sp_PackAllTablesOnline` and `sp_ReindexOnline` wait, before their final
    stage and before signalling `__OPFinalStage_<table>`, for the
    transactions on the table to end, up to their `Timeout` (7176, or 7189
    for a reindex); the table stays open to writes meanwhile. A reindex of a
    table with no index file does not wait.
  - **Full-text orders** (`fixtures/write/fts_tx_*`): a transaction's writes
    leave the orders alone until the commit, whatever the field (character
    or memo) and whether written by the API or SQL; `CONTAINS` on either
    connection reads the order as it was (its own deleted records
    excepted). A rollback leaves the `.adi` byte for byte as before. The
    commit applies each logged write's words in log order, the orders on
    memo fields before the others, each group by tag name; records commit
    in the order of their last write (the ADT free list's head is the last
    record written and deleted), a deleted slot keeps the record's last
    image, and the memo values of the images a record held between its
    original and its end are freed. A change pending in a handle at
    rollback writes nothing, memo values included. With the table open
    exclusively, `AdsReindexFTS`, `AdsReindex`, `AdsPackTable` and
    `AdsZapTable` are 5063; shared, the first two are 5067.
  - **A quirk of the original's**: after a script that commits an inner level
    of nested begins of its own and leaves one, the next `COMMIT` or `ROLLBACK`
    does its work but answers 5047 (`txscripts.json`, `/* sequence */`).

tflip does all of this but where KNOWN_ISSUES says otherwise.

## What follows for tflip

The decisions so far: match the original's behaviour and files, be atomic
after a crash, and work in one process (locally and in `tflipd`) first.
The probes change the picture:

- **For the local server, the original's behaviour is no transaction.**
  Matching it is cheap (the count and its errors). Doing more there,
  holding writes back until the commit, makes tflip's files differ from
  the original's in the middle of a transaction, and makes clients that
  rely on the local server writing at once behave otherwise.
- **For `tflipd`, the original to match is the remote server**, which has
  real transactions: it marks records in the files and keeps their new
  data in its log until the commit. The log's format is the server's own,
  and need not be tflip's. (An earlier reading of these probes, from
  checksums alone, took the server to write through and undo; the bytes
  show otherwise.)

## Decisions

Decided: tflip's local server has real transactions too, as the remote
server's; a setting turns them off for the original local server's
behaviour (a count, writes at once), for clients that rely on it.

Decided too: locally and in `tflipd`, transactions work as the remote
server's do, with a log of tflip's own format (only tflip reads it), so
the files match the original's: records marked in the files, their new
data in the log until the commit, appends and memo values written at once.
(This was first decided as "write through and undo", from a misreading of
the probes; the decision was to do as the remote server does.) `tflip.ini` holds the switch to the
original local server's behaviour; real transactions are on by default.


## Plan

Transactions as the remote server keeps them, in the engine, so the local
server and `tflipd` share them; checked against the `tx_*` fixtures byte
for byte (the files after each step and at the end; mid-run `.adi`
snapshots are stale and not compared, and the mark's second byte is the
server's own number). Each phase is committed when its tests pass.

### 1. The count, the switch and the API (no log yet): done

- **A transaction per connection** in the engine (`tflip_engine::
  transaction`): its nesting count, its savepoints by name (case
  insensitive), and later its log. `AdsBeginTransaction`,
  `AdsCommitTransaction`, `AdsRollbackTransaction(80)`,
  `AdsCreateSavepoint`, `AdsInTransaction`, `AdsGetTransactionCount` and
  `AdsFailedTransactionRecovery` as probed: 5047 out of sequence, 7129 for
  an unknown savepoint, handle 0 for every connection, a disconnect rolling
  back.
- **SQL**: `BEGIN TRAN[SACTION]`, `COMMIT [WORK]`, `ROLLBACK [WORK] [TO
  [SAVEPOINT] name]`, `SAVEPOINT name` on the connection's transaction (7200
  wrapping 5047 and 7129); `SET TRANSACTION` refused in one (5063).
- **The switch**: `tflip.ini` `[transactions] mode = real | count`; real
  by default. `count` gives the original local server's behaviour: the
  count and its errors, writes at once, SQL's statements accepted in any
  order (`tools/ace/transactions/tx.c`'s local answers). `tflipd` takes it
  from its command line.
- **The protocol**: operations for the transaction calls (a new protocol
  version); `tflipd` keeps a client's transactions in its session.

### 2. Marks, blank appends and the log: done

- **The log**: what the transaction wrote, record by record, in memory
  and on disk (see phase 4), written before the table files change.
- **Writes in a transaction** (`table/write.rs`, through `Pending`):
  - a change or delete of an existing record writes only the mark
    (ADT: the status byte's bit 7 and a transaction number in the next
    byte, `84 xx`; DBF: bit 7 of the deletion byte) and puts the new image
    in the log; the record's lock is held to the end
    (`AdsUnlockRecord` succeeds and keeps it);
  - an append writes a blank record with its mark (ADT `11 xx`, the
    header's count; DBF the deletion byte `0` and the count), and its
    memo values at once, as the fixtures show (ADT memo values of changed
    records too, DBF ones at the commit);
  - the orders get the new keys beside the old ones (both are found while
    the transaction runs), so unique orders refuse others the keys it
    took (7057).
- **Reads**: the connection that owns the transaction reads its records
  from the log (values, keys, counts); others read the files, skip the
  blank appended records, and see the old keys only.
- **Illegal in a transaction** (5063 with the probed texts): pack, zap,
  reindex, `AdsSetTableTransactionFree`; transaction-free tables
  (`AdsSetTableTransactionFree`, `sp_IgnoreTableTransactions`, a
  dictionary's property) written at once as now.

### 3. Commit and rollback: done

- **Commit**: each logged record's new image written into the file
  unmarked (deletes as deleted: ADT onto the free list, DBF `*`), the old
  keys taken out of the orders, the locks released, the log deleted.
- **Rollback** (also of a connection that closes or a client that goes
  away): marks cleared, appended records deleted (ADT onto the free list,
  DBF `*` with a blank memo reference), their memo blocks freed (ADT
  allocator, FPT next free block; a DBT keeps its length), the new keys
  taken out, the locks released, the log deleted.
- **Savepoints**: a rollback to one undoes the log's entries after it, as
  a rollback does, and keeps the transaction and its count.
- **Tests**: `writes.rs` replays the `tx_*` scenarios: `open2` (a second
  connection), the transaction commands, and `snapshot` compared with
  `snaps/` (but `.adi` files, and the bytes in `snaps-volatile.json`);
  `remote_twins!` runs them through `tflipd`.

### 4. The log on disk and crash recovery: done

- **The log** (`tflip_engine::txlog`, whose documentation gives the
  format): a file per transaction in the log folder (`tflip.ini`'s
  `[transactions] log`, else `TFLIP_TX_LOG`, else `tflip/log` in the
  user's state folder; `tflipd`'s is `log` in its state folder), made at
  its first write and deleted when it ends. Each write logs the record's
  state (its bytes before the transaction and now, whether deleted, a DBF
  record's memo values waiting for the commit), synced before the table
  files change; a rollback to a savepoint and the commit are logged too.
  The transaction holds a lock on its log, so a log nobody holds is a
  crashed transaction's.
- **Recovery**, as the help describes the server's at its start: a log
  that reached its commit is committed, any other rolled back. It runs when
  a table is opened (`tflipd` also when it starts) and for
  `AdsFailedTransactionRecovery` (on the local server and every server
  connected to; 7036 if a table of the transaction is open elsewhere, and
  the log waits). Each table is opened exclusively, its records that are
  still marked committed or rolled back as a running transaction's are,
  and its index files are rebuilt, since a crash can leave keys half
  moved. A record is written before the memo blocks it gives up are
  freed, so a crash in between loses blocks rather than freeing them
  twice.
- **Pending changes**: the commit writes the changes the connection's
  tables have pending into the transaction, and a rollback takes them back,
  as the original's server does (`tx_pending`).
- **Tests**: a process of the test binary writes in a transaction (two
  tables, a savepoint rolled back to, changes, appends, deletes, memo
  values) and ends without finishing it, or in the middle of its commit (a
  fault point in debug builds); recovered (by the call, or by the next
  process to open a table), the tables and memo files are byte for byte as
  when the same transaction is rolled back or committed, and their orders
  hold the same keys.

### 5. Several processes, SQL, the rest of the probes: done

- **Appends** logged, then written blank and marked at `AdsAppendRecord`
  (a crash leaves no empty record); a cancelled one kept until the
  transaction ends; DBF memo values on records without a block written at
  once (`tx_append`, `tx_dbf_memo`, `tx_vfp`).
- **Several processes**: another process reads a transaction's records as
  they were (the file has them so), passes its new keys by in walks and
  seeks, skips its blank appends, and cannot lock what it wrote; a test
  runs a transaction in a second process.
- **Failures in the middle of a write**: keys are built before anything is
  logged; a table closed in a transaction leaves it its record locks and
  the file open until it ends; an open waits for a recovery another thread
  runs.
- **SQL**: `SET TRANSACTION`'s modes, statements passing by locked records,
  as probed (`tx_sql`).
- **The index files after a commit** match the original's (the replays
  compare `after/`, written once the connections close).

Triggers in transactions of their own, procedures, scripts and referential
integrity inside a transaction follow the remote server's probes above.
