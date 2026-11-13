# Known issues

Where tflip behaves differently from the original engine, or does less,
as far as it is known, and not on purpose (the differences on purpose are
in [DIFFERENCES.md](DIFFERENCES.md); bugs in the original engine itself are
in [ADS_BUGS.md](ADS_BUGS.md)). Entries say what the probes of the
original engine showed, where that is known; what is planned about them is
in [TODO.md](TODO.md). Until there is a bug tracker, this file is it.

## Functions not implemented

Every function of `ace.h` and of the unpublished interface is exported.
None of them is a stub any more; some are implemented in part and fail
with `AE_NOT_IMPLEMENTED` for what is left. The code marks the calls it
accepts and ignores with `stub!` (see
[Settings that are only kept](#connections-and-configuration)); each
reports its first call on the `fixme` channel, which is on by default
(`TFLIP_DEBUG`, in the style of Wine's `WINEDEBUG`). In part are:

- **Writing to dictionaries**: the remaining `AdsDDSet*Property`
  properties (those the engine serves read-only).
  `AdsDDDeployDatabase` deploys a dictionary to a folder of free tables as
  the original does (`fixtures/dictionary/deploy`), in the client process
  whatever the server type, making the new dictionary from the source's
  objects, `usValidateOption` 2 moving the records that break constraints
  to fail tables, and tables in subfolders made with their folders. It
  carries the rights granted to `DB:Public` on views, procedures, links and
  publications, and drops those on functions, packages, users, groups and
  the database, as the original does. Rights
  granted to `DB:Public` on a table fail with 5136 after the other objects
  are added, as in the original, and a subscription comes with its queue's
  files. It fails with `AE_NOT_IMPLEMENTED` for a field's validation
  message, for fields of a DBF table with limits (the original drops them
  in part), and for moving the records of a table with memo fields to a
  fail table. A source with an administrator password gives the
  deployed dictionary that password, but under the source's dictionary
  key, where the original makes a new one.
  `AdsDDRenameObject` renames what `sp_RenameDDObject` renames, as the
  original does (`fixtures/sql/spddrenameapi`; fields, indexes and index
  files are refused with 5136 by both), and a dictionary DBF table's memo
  and index files (`fixtures/sql/spddrenamedbf`: every index file of the
  table moves to the new name, a table that is open or lacks a file is
  refused with 5067, and a rename that stops part way with 5020 keeps what
  it moved), and publications and subscriptions
  (`fixtures/sql/spddrenamerepl`: what names a publication follows it, a
  subscription's queue table keeps the name it was made with, and a
  renamed subscription is deleted without it);
  and through it `AdsDDMoveObjectFile`'s errors still come as 7200 where
  the original answers a plain 5132 or 5020.
  A dictionary-write precondition or validation error surfaces through the SQL
  error channel (code 7200, the real ACE code kept as its native error) rather
  than as the engine's raw ACE code, for most of the dictionary-write calls
  (`AdsDDCreateLink` and `AdsDDAddView` answer the original's code). Creating a dictionary, moving an object's files
  (`AdsDDMoveObjectFile`, of an ADT table or an index file; other tables
  and objects fail as not implemented), adding and
  removing tables, views, index files and stored procedures, creating and
  removing triggers, adding and removing referential-integrity rules, creating,
  dropping and modifying links
  (`AdsDDCreateLink`/`AdsDDDropLink`/`AdsDDModifyLink`), setting object access
  rights (`AdsDDSetObjectAccessRights`), creating and deleting user
  groups and
  users, users' group membership, permissions (on any object type, columns and
  the grant option included), and most property setters are done (see the
  changelog). Referential integrity is verified against the `dictionary_ri`
  fixture, built on the client engine, and tflip reproduces the engine's
  create-time preconditions (5131 for no primary key, 5143 for a parent tag that
  is not the primary index, 5132 for a missing table or child index); setting a
  table's primary key (`ADS_DD_TABLE_PRIMARY_KEY`), which RI needs, is done too.
  (The evaluation *server* was seen to fail/crash creating RI, but the client
  engine does not — see `docs/ADS_BUGS.md`.) Group membership and permissions are read and
  written with the dictionary key, recovered from the connection's login
  (the administrator's password or a user's own secret), so a dictionary
  with an administrator password is written as the original writes it
  (`fixtures/sql/spddadminpw`); the subscription writers use the same key
  but are not replayed under a password. The password is set as the
  original sets it (`sp_ModifyDatabase`'s `ADMIN_PASSWORD`,
  `AdsDDSetDatabaseProperty` 101: by `ADSSYS` only, the key kept), but
  other tflip connections to the dictionary keep the old password and fail
  their next reload (what the original does for a connection opened
  before the change is not probed).
- **Replication, the replication engine itself**: subscriptions are created
  with their replication queue table, but tflip does not run replication (apply
  the queue, forward updates). Articles, publications and subscriptions are
  created, read, changed and deleted as the original does
  (`fixtures/sql/ddrepl`, `ddrepl2`).
- **`AdsGetKeyFilter`** is done: it writes the current record's key as a
  filter, byte-for-byte as the engine does — the unique index whose tag sorts
  first, or else the table's AutoInc field (none → 5041); `field=value AND ...`
  with text quoted (not escaped), doubles as `2.250000E+000`, Money with four
  decimals, dates as `datetime'YYYY-MM-DD HH:MM:SS'`, `field=null` off a record;
  `ADS_FILTER_FORMAT_ODATA` (1) joins with commas (a single value bare), uses a
  `T` in dates and the `M`/`L` type suffixes; options above 3 fail with 5019.
  Left: preferring a dictionary table's named primary key over the
  alphabetically-first unique order, and a `pucValuesTable` values file.

## Transactions

Transactions are counted and nested, with savepoints, as the original's
remote server keeps them, through the API and SQL (`BEGIN TRANSACTION`,
`COMMIT`, `ROLLBACK [TO SAVEPOINT]`, `SAVEPOINT`), on the local server and
on `tflipd`; pack, zap, reindex and `AdsSetTableTransactionFree` are refused
in one (5063), as there. `tflip.ini`'s `[transactions] mode = count` (or
`tflipd --transactions count`) keeps them as the original's local server
does instead. But (see [TRANSACTIONS.md](TRANSACTIONS.md#plan)):

- **After a crash**, a transaction is finished or taken back from its log
  on disk when a table is next opened, but not byte for byte as the
  original's server recovers (which was not probed): the tables' index
  files are rebuilt, and memo blocks the transaction had written may be
  lost. Recovery needs the transaction's tables open nowhere else; until
  then it waits (and
  `AdsFailedTransactionRecovery` fails with 7036). Logs are found in the
  log folder only: processes with different log folders do not recover
  each other's transactions.
- **Index files that cannot be written** in the middle of a transaction's
  write leave the record the transaction's, with its keys half moved; the
  rollback (or a recovery, which rebuilds the indexes) puts it back.
- **Other processes** read a transaction's records as they were, pass its
  new keys by and skip its blank appends (as other connections of its
  process do), and cannot lock what it wrote; a process opening a table
  in the instant another one recovers a crashed transaction of it can
  write it before the recovery (in one process, the open waits).
- **A DBF record's own memo values** set in a transaction are written at the
  commit where the record has a memo block, as the original's server writes
  them (where it has none, at once: `tx_dbf_memo`); until then the
  transaction reads the values the record had. An ADT table open
  exclusively holds back, the same way, the memo values that need no new
  block (they fit the blocks of the one they replace, or clear it): the
  commit writes them in place, and until then the transaction reads them
  but SQL and other handles' reads in it see the old ones
  (`tx_memo_exclusive`, `tx_memo_sizes`). A table with a full-text order
  and a compressed field writes them at once as before.
- **A handle's image after a rollback** (`fixtures/write/tx_cursor_after`):
  the handle left on a record that a rollback to a savepoint freed keeps
  its image (not deleted, its values) and, in an ADT table, its record
  count until it appends; a whole rollback leaves an ADT handle on a freed
  slot at BOF with the record number past the last slot, as the original
  does. Not matched: the stale count of an ADT handle after a transaction
  that also deleted a record through it (the original refreshes it at the
  commit); and the images of other handles on the record, which tflip's
  see at once (see "Other handles see changes at once").
- **A DBF memo value changed while another handle holds the record's
  image** goes to new blocks once in a Clipper, FoxPro or VFP table (in an
  `.fpt` the new blocks are taken first and the old ones given back after;
  a `.dbt` appends), outside a transaction and at a commit, as in the
  original (`fixtures/write/memo_other_handle`). A handle holds the image
  from the moment it moved onto the record; a `goto` to the record it is
  on does not renew it in tflip (the original's not probed). On the remote
  server the original puts every changed ADT memo value in a new block,
  even with one handle alone; tflip writes it in place (see TODO).
- **Memo and index files in the middle of a transaction** are not compared
  with the original's: its server keeps their free lists and pages in
  memory meanwhile. Only the files after the transactions end are.
- **A statement's lock retries**: an `UPDATE` or `DELETE` that meets a
  record another holds locked passes it by at once and goes on, failing
  with 5035 at the end, as the original does; the help says the
  original's SQL engine tries the lock several times first.
- **`AdsFailedTransactionRecovery`** recovers on the local server too,
  where the original's has nothing to recover; with a path, it fails with
  5033 if no connection is on it.
- **In the count mode**, `AdsBeginTransaction(0)` adds one to each
  connection; the original's local server once added two to one of them,
  by a rule not worked out.

## Errors

- **Wrong handles in the last error**: calls given a value that is no
  handle, 0, or a handle of the wrong kind return what the original returns
  and leave its last error, after no error and after others
  (`fixtures/errors/handles.json` and `handle_order.json`, from
  `tools/ace/errors/`): a table call's check keeps an error left before,
  `AdsGotoTop`'s, `AdsCloseTable`'s and `AdsCloseSQLStatement`'s replace it.
  Probed for the calls those fixtures make (each check's own rule); the
  other calls take the rule of their kind of check, unverified one by one.
  The original hands a closed table's handle to the next object it opens
  (a statement); tflip does not reuse handles.
- **An index order for a connection**: the original's transaction calls
  take an index order's handle (and act on its connection: `AdsBeginTransaction`
  succeeds); tflip's fail with `AE_INVALID_CONNECTION_HANDLE`.

- **Details in error texts**: `AdsGetLastError` gives the original
  engine's text for the code (`fixtures/errors/texts.json`) and the details
  it adds after a failure: every command of the write fixtures compares its
  text (the file not found, the order a unique key violates, the field, the
  scope, `Parameter 3`, why a restructure or a field definition is refused,
  ...; `fixtures/write/error_details` for the rest). Not every call's
  details are probed: `Parameter N` after `AE_INVALID_POINTER` and
  `AE_INVALID_OPTION` is given by the getters of tables, fields, orders,
  connections, cursors, statements and the dictionary, the field, record
  and binary setters and their `W` forms, the SQL text calls, the filter,
  key and scope calls, and the calls the fixtures refuse an option of
  (`fixtures/errors/param_n.json`), the administrator getters
  (`param_n_mg.json`) and the setters of statement parameters, but not
  `AdsSetDouble` on one. The dictionary getters' reasons for refusing an
  object or a property (5132, 5126, 5133, 5054) are the original's for the
  kinds of object the probe has (groups with rights granted and RI rules
  among them), not yet probed for links, views, triggers, procedures,
  publications, subscriptions and articles, nor 5054 for a user other than
  none; `AdsGetPermissions` of `ADSSYS`
  fails with 5132 where the original's succeeds. One text
  differs: `AdsConvertTable` of a Clipper table whose General value cannot
  be read returns 7019, but the original's last error is then 5169 (it
  scaled the copy back to the client) (`known.json` of `converts_dbf`).
- **The raw setters** (`AdsSetFieldRaw`, and `AdsSetTimeStampRaw` on a
  table, which sets any field's bytes as the original does): a Numeric
  field of an ADT table reads them as a number (5070, 5179), a VarChar
  value keeps its length bytes (one longer than the field is cut 2 bytes
  short), and a DBF table's numeric and date fields are padded with blanks
  (`fixtures/errors/param_n.json`, the `a`, `x`, `y` and `v` rows). Not
  probed: the stored forms of the other types (NVarChar, VarBinary, Raw,
  Guid, Money, DBF memos).
- **A group's rights properties** (1001 to 1004) list tables, views and
  procedures as the original does, without links or functions; a group's
  table rights are listed only through the permissions the connection's
  key deciphers (`AE_NOT_IMPLEMENTED` otherwise), and a permission revoked
  to nothing is left out, where the original lists it as `t:;`.
- **A refused candidate key's CDX counter**: appending to a VFP table, a
  write a candidate order in another CDX file refuses (7057), then
  `AdsCancelUpdate`, leaves that file's change counter one higher in tflip
  than in the original (found making `fixtures/write/error_details`; not
  in that fixture).

- **Dates before the year 1**: `AdsConvertJulianToString` gives some Julian
  days before 0001-01-01 a day earlier than the proleptic Gregorian date
  (1721425 is `00001230`, others not), by a rule not worked out; tflip gives
  the proleptic date. Huge day numbers (1e8) come out garbled there; tflip
  writes `****` for years past four characters.

## Connections and configuration

- **Login passwords are checked**: a dictionary connection verifies the
  password as the original engine does (probed, `tools/ace/dd/login.c`):
  `ADSSYS` against the administrator password that recovers the dictionary key,
  every other user against its stored login secret. The user name matches
  whatever its case, the password exactly; a wrong, empty or missing password
  for a user that needs one, and no user where the dictionary requires logins,
  fail with 7078. Free-table connections take any user, as the original does.
  Disabled logins are the original's (`fixtures/sql/ddlogins`: 7109 with
  the dictionary's `LOGINS_DISABLED_MSG` for all but `ADSSYS`, 7113 for a
  user whose own logins are disabled, both after the password's 7078).
  The bad-login lockout (`ENFORCE_MAX_FAILED_LOGINS`) is `tflipd`'s only,
  as in the original's remote server (`fixtures/events/lockout_*.json`).
  The local server counts no failed logins, whatever the setting
  (`fixtures/events/lockout_local.json`); its 7078 for a known user,
  ADSSYS or no user names `axServerConnect`, one for an unknown name does
  not. Not probed: the lockout of Internet connections (which count
  without `ENFORCE_MAX_FAILED_LOGINS`). The old password is required to
  change a user's own password as in the original
  (`fixtures/sql/spddpwchange`): 5054 through `sp_ModifyUserProperty` and
  `AdsDDSetUserProperty` (`ADS_DD_USER_PASSWORD`) unless
  `REQUIRE_OLD_PASSWORD` is off, 5097 for a wrong old one given with
  `ADS_DD_CURRENT_USER_PASSWORD` or `sp_ChangeCurrentUserPassword`; ADSSYS
  is never asked, and only ADSSYS changes ADSSYS's. A login is checked
  against a stored secret whose check is as long as the password, so a
  one-character password is checked by one byte: a wrong password passes
  about once in 256 tries in tflip (whether the original's does too is not
  probed). The
  original's remote server sometimes resets the circuit (6624) after
  failed logins, and then every connect of that client process; it was
  not reproducible (the same script ran clean and not), and `tflipd`
  never does it.
- **`ads.ini` aliases are not resolved** — as in the original engine. Black-box
  probing (`tools/ace/config`) showed `ace32.dll` opens `ads.ini` during a
  connect but does not act on its `[Databases]` aliases: a connect name is
  always a folder path relative to the current directory, so an alias resolves
  to the folder `<name>` there (`.\<name>` under Wine), not the aliased path. Alias resolution is done one layer up, by
  the TDataset/ARC Delphi client, which hands the engine an already-resolved
  path. tflip faithfully does the same (its own settings live in `tflip.ini`);
  reading `ads.ini` aliases would be a divergence from the engine, not parity.
- **Connection string options**: `AdsConnect101` refuses the keys and
  values the original refuses (`fixtures/config/connect101*.json`), and acts
  on `TableType`, `CharType`, `ReadOnly`, `Shared`, `LockMode` (for
  `AdsOpenTable101`), `DateFormat`, `Exact`, `Collation` and
  `TrimTrailingSpaces`, but: `ShowDeleted`, `Epoch`, `Decimals`,
  `SecurityMode`, `IncrementUserCount`, `SQLTimeout`
  and the pooling keys are checked and not acted on (no probe showed an
  effect); `CheckFreeTableAccess` (and the connect option 0x2000) is sent to
  `tflipd` and acted on there as locally; `TrimTrailingSpaces` is probed on `AdsGetString` of Char fields
  only; `phConnectionOptions` is not written (the original's is a nonzero
  handle `AdsGetHandleType` calls a table); and where the original lacks
  the OpenSSL libraries it fails `FIPS=TRUE` and the TLS keys with 5214,
  which tflip accepts.
- **Settings that are only kept**: `AdsSetProperty` acts on 39 (the next
  `AdsVerifySQL` checks the syntax alone) and 87 (strong encryption is
  available) and accepts 27, since tflip caches no error codes; its other
  ids are accepted and ignored, where the original refuses some (12345
  with 5101, `Parameter 2`; which ones is not probed); the lock type given to open, create or restructure a table
  (`AdsOpenTable90`, `AdsCreateTable90`, `AdsRestructureTable120` and their
  older forms) is ignored; `AdsRegisterCallbackFunction`,
  `AdsRegisterSQLAbortFunc`, `AdsRegisterProgressCallback` and
  `AdsRegisterUDF` never call back;
  `AdsSetSearchPath` is kept but not searched, and `AdsSetDefault` kept
  but not used to open or create tables; `AdsStmtEnableEncryption` is kept
  and changes nothing, as in the original; `AdsStmtSetTablePassword` is
  applied to the tables a statement's SQL opens (not probed for an `ALTER
  TABLE` restructure of an encrypted table).
- **Collation names are checked against the repository**: `AdsSetCollation`
  and `AdsStmtSetTableCollation` refuse a name as the original does (7147; the
  statement also takes `ANSI`, `OEM` and `:` with an ICU locale, 1564 for one
  without a collator), the same on a free-table and on a local or remote
  dictionary connection. Without `adscollate.adt` beside the DLL tflip cannot
  tell a repository name is wrong and keeps it. The ICU locales are the ones
  probed (`icu_collators.txt`: the locales of ICU 4.0's data and the ADS
  variants, and the ISO 639-2 codes of a list of languages): a locale or code
  outside both that the original opens would be refused. No dictionary-defined
  collation was found in the original (a dictionary takes the same names), so
  none is modelled.
- **Finding connections**: `AdsFindConnection25` matches a connection's
  folder regardless of case and of a trailing separator, which was not
  probed (the folder itself, and not a table in it, was).
- **Server discovery without details**: `AdsFindServers` discovers servers by
  IPv4 multicast and fills the address, port, multicast-address and server-name
  columns, but leaves the ten detail columns (serial number, version, owner,
  user count, …) blank — the original gathers those by opening a full client
  connection to each server, which tflip has no remote-server client to do. It
  behaves like the original's `ADS_FS_MULTICAST_ONLY` for every option. (That
  connect-for-info protocol was reversed black-box but deliberately not
  implemented: `tools/ace/findservers/detail-protocol.md`.) (The
  empty-result case is a deliberate difference, not a gap — see
  `docs/DIFFERENCES.md`.)
- **Open tables**: `AdsGetNumOpenTables` and `AdsGetAllTables` count the
  tables the client opened; the original engine counts tables of its own
  too (its error log, ...).

## Management functions

- **Management handles**: `AdsMgConnect` hands out a connection (its
  `AdsGetHandleType` is `ADS_CONNECTION`, as the original's); the `AdsMg*`
  functions take any connection handle, as the original also does, and
  refuse their arguments in the original's order and words
  (`fixtures/errors/param_n_mg.json`, replayed locally and on `tflipd`; the
  rows that depend on what the server has in use, 7050 and 7051, are not
  compared). The original's `AdsMgConnect` takes a server's name and ignores
  the rest of the path; tflip's takes what `AdsConnect` takes, the share
  included for `tflipd`.
- **Activity**: `AdsMgGetActivityInfo` reports the connections, work
  areas, tables and indexes in use as the original's local server does,
  but not its other values: it reports 1 logged error and "max used"
  values (work areas 3, tables 2, indexes 2, locks 2, queries 1) before
  anything is opened, which tflip cannot account for; tflip reports the
  counts in use as the maximum (0 for connections, as the original) and
  0 for the rest. For `tflipd` the counts are the connection's session's,
  not the server's.
- **Users**: `AdsMgGetUserNames` lists one user per connection, with the
  OS user's name and the program's file name, as the original's local
  server does; for `tflipd`, the connections of the client's own session,
  not every client of the server. `AdsMgGetOpenTables` and
  `AdsMgGetOpenIndexes` likewise list the session's tables and indexes.
- **Configuration**: `AdsMgGetConfigInfo` reports the configuration
  parameters the original's local server reports, its defaults (20
  connections, 100 work areas, 50 tables, ...), though tflip has no such
  limits; the memory they take, which the original reports from its own
  allocations, is 0 throughout.
- **No error log**: the original's local server logs errors in a table of
  its own (`C:\ads_err.adt`), which shows in its management functions:
  as a logged error and an open connection, and as an open table once it
  is used. tflip keeps no error log.
- **Locks, threads and users**: `AdsMgGetLocks` and `AdsMgGetLockOwner`
  report the record locks the session holds (record numbers; the owner as
  the session's user); `AdsMgGetWorkerThreadActivity` reports no threads,
  as the original's local server; `AdsMgKillUser(90)` and
  `AdsMgDumpInternalTables` succeed without doing anything, and so does
  `AdsDisableLocalConnections`.

## Data dictionaries

What tflip reads of dictionaries is checked against the original
(`fixtures/dictionary/dd.json`, `fixtures/dictionary/variants/`, and the
`system.*` dumps of the dictionary fixtures). But:

- **Encrypted dictionaries**: an RC4 one (`ENCRYPT_DICTIONARY`) is read
  (`fixtures/adt/dictionary_encrypted/rc4.add`), and an AES one with its
  dictionary password (`DDPassword`), as the original reads it
  (`fixtures/aes/dict`): 7068 without it, 7060 with a wrong one, and its
  encrypted tables open with it; a connection without it opens one another
  connection of the process opened with it, as in the original. AES
  dictionaries are created and written as the original writes them,
  compared deciphered (`fixtures/aes/dictwrite`): records, memo values and
  the slabs' records behind their IVs, the index enciphered, tables armed
  after `ENCRYPT_NEW_TABLE`. But: the IVs are tflip's own (a process
  reserves a block of 1000 in the header, as the original does; see
  DIFFERENCES for the bytes after a memo value); and over `tflipd` only opening and reading an RC4 dictionary's table,
  and connecting to, reading and appending to an AES dictionary's table
  (the `DDPassword` is sent with the connect) were tried; `EncryptionType`
  is sent after the connect (`set_encryption_type`), so a table made over
  `tflipd` on an AES connection is laid out for AES, but
  `AdsIsTableEncrypted` right after `AdsEncryptTable` over `tflipd` still
  says 0 (not looked into). A table whose index is
  enciphered takes inserts, changes, deletes, `CREATE INDEX` (its first
  index file is made enciphered), `DROP INDEX`, `sp_Reindex`,
  `sp_PackTable` and `sp_ZapTable` as the original does, compared
  deciphered (`fixtures/aes/dictwrite`, `16_index_create` to
  `21_index_clear`), and a full-text order (`40_fts_create`, recorded in
  the dictionary with its options, `43_fts_dictionary`); a page of such an
  index that a write did not change keeps its bytes and IV, as in the
  original. `ALTER TABLE` and `AdsRestructureTable` of an encrypted table
  put it in the clear, set the old files aside as clear `.BAK` files and
  encipher the new table with its indexes in the clear, as the original
  does (`42_restructure`, `44_restructure_one`), and writes with an ADSSYS
  password are the original's (`45_adssys_password`). A restructure of a
  table with full-text orders rebuilds them with their options, in an AES
  or RC4 dictionary that encrypts too (`47_restructure_fts`). But the noise words a
  full-text order made by `sp_CreateIndex90` or `AdsDDAddTable` records in
  the dictionary are the index's own sorted list, not the given words
  followed by the defaults that `CREATE INDEX` records.
  `AdsDDCreate` with a non-zero `usEncrypt` makes an RC4 dictionary as the
  original does (`fixtures/aes/dictwrite`, `22_rc4_create`);
  `sp_SetDDEncryptionType` converts a plain, RC4 or AES dictionary to RC4,
  AES-128 or AES-256 as the original does (`fixtures/aes/convert`, compared
  deciphered; see DIFFERENCES for the frames of the rights granted to
  `DB:Public` and of memberships); the
  re-enciphered passwords of links and subscriptions (of every length,
  with and without a user, long connection strings) match the original's,
  compared deciphered (`c_Y_*`, `c_Z_*`, `c_W_*`). After
  `sp_ModifyDatabase`, or a conversion, of a dictionary of about 160
  records, its `.ai` is the original's (`fixtures/aes/convert`, letter V). `AdsDDCreate101` refuses an unknown key of its connect
  string with 5019, takes `Password` as the ADSSYS password and
  `Description` as the comment (`fixtures/aes/dictwrite`,
  `46_connect_keys`). `AdsConnect101` reads a key up to the next `=` as
  the original does, refusing an unknown one with 5019, but
  `=;ReadOnly=true;` (the original: 5019 with detail `=`), ` ;=x;Foo=1;`
  (the original passes it) and a known key whose value runs into the next
  item (`ReadOnly = true Foo=1`: the original's detail is `true Foo=1`)
  differ.
- **Encrypted properties**: the administrator's and users' passwords are
  read to check logins and recover the dictionary key (see Connections and
  configuration), and a subscription's password and connect string answer
  its getter; the getters do not return the other passwords. Permission
  rights are read, in `system.permissions` and by `AdsDDGetPermissions`.
  A user's group membership (`ADS_DD_USER_GROUP_MEMBERSHIP`) is read with
  the dictionary key from the connection's login; an anonymous connection
  to a dictionary with an administrator password cannot read it for a user
  in a real group (`AE_NOT_IMPLEMENTED`; a user in only the built-in
  `DB:Public` always reads).
- **Links** to other dictionaries are followed by SQL (`link.table` and
  `"path\dict.add".table` in `SELECT`, `INSERT`, `UPDATE`, `DELETE` and views,
  logging in as the link's user — `fixtures/sql/links.json`), by
  `sp_GetTables`/`sp_GetColumns`, and by the listings
  (`ADS_DD_TABLE_VIEW_OR_LINK_OBJECT`, `AdsFindFirstTable62`); the active links
  are counted (`fixtures/sql/linklist.json`). One corner of the active links is
  not matched: a statement that used a link before a listing left it inactive
  does not make it active again in the original (a new statement does); tflip
  makes it active again. Making a global link is refused to a user who is not
  the administrator or in `DB:Admin` and holds no `CREATE LINK` right (5054,
  `fixtures/sql/ddrights.json`).
  The procedures read the linked dictionary without logging in as the link's
  user, and skip one they cannot read (an encrypted one, a missing file) where
  the original's behaviour is not probed. Not probed either: a linked
  dictionary that exists but cannot be read (tflip fails with 5174 / 7008), an
  encrypted one (`AE_NOT_IMPLEMENTED`), a link whose password cannot be
  unwrapped because the connection recovered no dictionary key (an anonymous
  connection to a dictionary with an administrator password;
  `AE_NOT_IMPLEMENTED`), and links changed or dropped after a statement used
  them (tflip keeps the connection it opened to the linked dictionary until
  the connection closes). A link that is not global lives only on the
  connection that made it, as in the original, so other connections'
  `AdsDDDropLink`, and `AdsDDModifyLink` and `AdsDDGetLinkProperty`, do not
  find it.
- **Table listings with a mask** on a dictionary connection list the
  dictionary's folder; the original lists the process's current folder.
- **`ADS_DD_RI_PARENT_GRAPH` into a short buffer**: the original writes
  part of the rule's name and succeeds; tflip refuses it.
- **UTF-16 properties into a short buffer**: the original writes a stray
  byte after the first character; tflip does not (the lengths agree).
- **DBF tables in a dictionary** (Clipper, FoxPro, Visual FoxPro) are added
  (`AdsDDAddTable`, `CREATE TABLE`), opened by name and read and written
  through SQL as the original does (`fixtures/dictionary/dbftables`,
  `dbfcorners`, `fixtures/sql/dbfdict.json`, `ddbf.json`), also OEM tables (the
  character type is kept in the Table object), a VFP table's NULL-able fields,
  the refusals of a table type and character type (`ADS_DEFAULT` is no type to
  add), and a primary key and field rules in `CREATE TABLE`. What differs:
  - not probed: an ADI file added to a Clipper, FoxPro or VFP table (tflip
    refuses it as it does an NTX file to a FoxPro table, 7017), a collation
    of a VFP table in the dictionary, and the keys of a VFP OEM table's
    index (they are ordered as an ANSI table's, as is a free VFP table
    opened as OEM, which stays ANSI; a dictionary's OEM VFP table does
    convert its text, as the original's does).
- **Rights granted to `DB:Public`** are kept as the original keeps them, in
  the object's own slot (32 bytes enciphered with the dictionary key, with
  a salt and a checksum, checked on every read: 5127 for any change); API
  and SQL grants and revokes, `system.permissions`,
  `system.effectivepermissions`, `AdsDDGetPermissions` and access checks
  see them (`fixtures/dictionary/public.json`, `ops/public*`). A slot that
  does not check fails as in the original: opening the table (7207, the
  administrator's too, before its rights are looked at) or reading the view
  (7207), dropping the table (5127), `system.effectivepermissions` (5127),
  and for a user held to its rights `system.tables` and `system.columns` (a
  bad table), `system.views` (a bad view) and `sp_GetTables` (either) with
  5127 (`fixtures/sql/ddpublicbad`, `ddpublicbad2`). Bad slots of every other kind of object fail as in
  the original too: a user other than the administrator gets 5127 from the
  system tables of its kind (`storedprocedures`, `functions`, `users`,
  `usergroups`, `links`, `packages`, `publications` and `articles`,
  `subscriptions`, `views`, and `dictionary` and `tables` for the database;
  `indexfiles` and `triggers` for a bad table, `triggers` for a bad view,
  `sp_GetTables` for a bad link, for a user held to its rights), and the
  call of a bad procedure, function or package is 5054 in a dictionary that
  verifies rights, the administrator's too; `system.permissions` and
  `system.effectivepermissions` lose the rows that read the slot as the
  original does (the first fails the statement when nothing was shown yet,
  `ORDER BY`, grouping and filters on the rights columns read them all, and
  each later lost row is paid for with a copy of the first row of its block
  of ten), but the original reports 0 records and numbers the rows by their
  place in the grid, which tflip does not (`fixtures/sql/ddpublicbad3`,
  `ddpublicbad4`, `ddpublicbad5`). Not matched: with several bad objects, a
  filtered statement can show one copy more than the original (`WHERE
  Grantee = 'DB:Public'` and `WHERE "Execute" = 1` on the permissions of
  ddpublicbad4's dictionary); a bad table that is not the first the user
  lists makes the original's `system.tables` and `system.columns` for a
  user held to its rights repeat rows (200 and more, count 0) where tflip
  says 5127, and without verified rights the original's `system.tables`
  for an ordinary user loses the bad table's row where tflip lists it; a
  bad package makes an ordinary user's `system.functions` repeat the first
  function without end; and an ordinary user's (held to its rights)
  permissions and effective permissions on a dictionary with a bad table
  slot (5127 for most filters, 15 rows for `WHERE Name = 't2'`) and its
  `sp_GetColumns` of `t2` or a good view (0 rows in the original, where
  tflip lists the view's column; `fixtures/sql/ddpublicbad10`, recorded,
  not replayed). A `DB:Admin` member and ADSSYS get the effective
  permissions of the other rows and 5127 for the bad table's, and
  `sp_GetColumns` of a good view lists it (`fixtures/sql/ddpublicbad9`);
  `WHERE Object_Type = 4` lists the first table's columns only, bad slot or
  not (a `Parent = '…'` conjunct names another table;
  `fixtures/sql/ddeffperm`). `EXECUTE` of procedures, functions and packages is checked in
  SQL for a user held to its rights (5054; the calls inside a procedure's
  or function's body are not), as in the original
  (`fixtures/sql/ddexecrights`); a function called inside a view or a
  trigger was not probed. `GRANT
  ALL` and `REVOKE ALL` to `DB:Public` by a user whose grantable rights are
  cut to nothing write no slot, and rights held with the grant option on
  columns only are not for it to give to a table's slot
  (`fixtures/sql/ddpublicgrant`).
- **`SHOW PLAN FOR GRANT …`** (or `REVOKE`): the original answers with a plan;
  tflip refuses it as it does every statement it shows no plan of.
- **The dictionary's constraints, defaults, validation and triggers on write**
  match the original's remote server (`fixtures/dictionary/constraints.json`)
  and, with `[transactions] mode = count`, its local one
  (`constraints_local.json`, and `rules.json` for their order against
  referential integrity, defaults, minimums and maximums as expressions, and
  which of them `AdsDDSetFieldProperty` refuses), with corners not probed:
  whether a table-API record's own transaction also takes back what a
  RESTRICT refusal's triggers wrote, and `NO TRANSACTION` triggers through the
  table API (tflip treats both as SQL does); minimum and maximum on character
  fields under a collation (compared as bytes); a date default or bound read
  in the connection's current date format (the dictionary stores the text as
  given); and the record validation evaluated with the "exact" setting the
  table had at open. A default, minimum or maximum that is an expression is
  read with tflip's expression engine on the record (an expression the
  original reads differently, such as `RECNO()`, which an SQL `INSERT`'s
  default reads as 0 there, may differ); a minimum or maximum that reads
  another field is read on the record being checked (the original seems to
  read it on an empty one); one whose expression fails as it is read (`1/0`)
  is taken as text where the original refuses to set it (`4420`). When a
  child refuses a cascaded change of a parent's key, the original leaves the
  parent as it was without transactions too; tflip puts the parent's record
  back (its memo fields as written), and leaves the children it re-pointed
  before the one that refused (the original was not probed with two). A
  cascaded delete runs as SQL, so a child's failing BEFORE DELETE trigger is
  not silent as it is through the table API (not probed). A record whose
  write a constraint refuses and that is then closed without
  `AdsCancelUpdate` is dropped, the appended slot staying as written empty (as
  for a unique-key refusal). In a table-API write, a trigger's `__new` reads a
  memo field as stored, not as it is being set.
- **`system.*` tables**: the readable ones match the original
  (`tflip_engine::system`), but `system.usergroupmembers` lists a user's real
  groups, and `system.permissions` and `system.effectivepermissions` fill their
  granted cells, only when the dictionary key is recoverable (see above;
  otherwise memberships fall back to `DB:Public` and grants to zero).
  A dictionary that verifies access
  rights enforces them per table (open needs the read right, and
  insert/update/delete need theirs — probed, `tools/ace/dd/rights.c`), matching
  the original; column rights are under Access rights in SQL. Still
  kept-but-unenforced are the client-side `AdsSetRightsChecking` and
  `AdsStmtSetTableRights`.
- **What a user sees of the dictionary** is the original's
  (`fixtures/sql/ddvisible`: the system tables and `sp_GetTables`,
  `sp_GetColumns` and `sp_GetProcedures` list what the user holds a right
  on, with the original's masks), but not probed: `system.publications`,
  `articles` and `subscriptions` for a user holding a right,
  `AdsDDFindFirstObject` of procedures,
  functions and packages (tflip lists them all there), `sp_GetColumns` on
  views, `system.packages` for a user holding a right, and a member of
  `DB:Admin` (treated as the administrator).
- **Views** in a statement's `FROM` that read one table's columns (some of
  them, renamed, with a `WHERE`, or of another such view) are live cursors
  on the table, as in the original engine (`fixtures/sql/views.json`), and
  so is a `SELECT` of one with a `WHERE` or an `ORDER BY`, unless that makes
  it static. Other views are read into a static cursor, a view of a `SELECT`
  in `FROM` among them (the original's is live, as a `SELECT` in `FROM` is:
  below). The plan of a statement on a view whose `WHERE` the statement's
  `WHERE` joins writes the AOF `a .AND. b`, the statement's first; the
  original writes `(a).AND.(b)`, the view's first.
- **Views, procedures and databases made with SQL** (`CREATE VIEW`, `DROP
  VIEW`, `CREATE`, `ALTER` and `DROP PROCEDURE`, `CREATE DATABASE`) write the
  dictionary files byte for byte as the original does, with its refusals and
  its errors' words and places (`fixtures/sql/ddviews`, `ddprocs`,
  `dddatabase`), and so do `AdsDDAddView` (which checks the statement's
  tables and columns, 2115 for a derived table or subquery with an `ORDER BY`
  and no `TOP`, 2117 for the statement's own) and `AdsDDAddProcedure`, which
  also put the object in the slot of a deleted one. `DROP VIEW`, `DROP
  PROCEDURE` and `DROP TABLE` take the permissions on the object with it, as
  the original does (not probed through `AdsDDRemoveView`, `…Procedure` and
  `…Table`, which share the code). But:
  - `CREATE VIEW`, `DROP VIEW`, `CREATE PROCEDURE` and `DROP PROCEDURE`
    are held to the right to create or drop them (5054, see "A user who
    is not the administrator" below); `CREATE DATABASE` is refused to no
    one, as in the original;
  - a procedure's parameters are checked as the fields of `CREATE TABLE`
    are, which differs from the original's checks of lengths (a `VARCHAR`
    or `VARBINARY` of more than about 60000 characters, an `NCHAR` or
    `NVARCHAR` of more than 32500, a `NUMERIC` with fewer than 2 more
    digits than decimals are 5012 there; tflip's `CREATE TABLE` does not
    check them either);
  - `ddview` of a text that is no `SELECT` fails with 2115 where the
    original answers `EXECUTE PROCEDURE …` with 2141 and `(SELECT …)` with a
    syntax error at the end; a procedure or view over a bound table, made on
    a free connection, is not refused with 5159 for the table;
  - `CREATE PROCEDURE …; SELECT …` sent as one script, which the original
    runs, was not probed in tflip's script host;
  - a library path with a backslash first is stored as the original stores
    it, `......\path`, which looks like a defect there; one on another drive
    or server is 5020;
  - `CREATE DATABASE` makes the folders of the path only on the original's
    dictionary connections (not here), takes an absolute path where the
    original under Wine says 7040, replaces any extension by `.add`, and
    refuses `PASSWORD` that is not empty, with native 2200 (the original
    enciphers it into the file). `ENCRYPT TRUE` without an AES type (with
    `ENCRYPTIONTYPE RC4` or a `DDPASSWORD` too) makes an RC4 dictionary,
    `ENCRYPTIONTYPE RC4` alone a plain one (`41_rc4_sql`), and an AES
    dictionary, `ENCRYPT TRUE` or not, is made as the original makes it.
    `DDPASSWORD` alone does nothing, as in the original;
  - every object added takes a deleted one's slot as in the original
    (`fixtures/sql/ddslots`), but the replication objects (publications,
    articles, subscriptions) were not probed for it. Publications' and
    subscriptions' comments are (`ddrepl2`): a comment of as many blocks is
    written over the old one, else the old one is freed first.
- **A user who is not the administrator** is refused as the original
  refuses it, with 5054 and its messages (`fixtures/sql/ddrights`, the
  dictionary byte for byte): `CREATE VIEW`, `PROCEDURE` and `TABLE … IN
  DATABASE` without the right to create them, `DROP VIEW`, `PROCEDURE` and
  `TABLE`, `ALTER PROCEDURE` and `TABLE`, `CREATE` and `DROP INDEX` and
  `TRIGGER` (on a table or the database), `sp_CreateUser`, `sp_CreateGroup`,
  `sp_CreateLink`, their drops, `sp_AddUserToGroup` and the integrity rules;
  the creator holds all rights on its object with the grant option; a plain
  `CREATE TABLE` without the right makes a free table, as there; and
  `system.triggers` shows such a user the triggers of the tables it may
  alter. A `GRANT` or `REVOKE` is held to the grant option (5054
  `Permission to grant or revoke one or more permissions on the table [t3]
  is not granted.`; `ALL` is cut to what it holds with it; the right to
  create objects is never given `WITH GRANT`), a member of `DB:Admin` is
  held to nothing, and the `AdsDD` calls that add an object are held to the
  right to create it, answering 5054 themselves (`fixtures/sql/ddgrantopt`,
  `ddbuiltin`). As in the original, a member of `DB:Admin` that grants or
  revokes clears the flag (Info2) of every Permission object, for a reason
  not known. A user who is not the administrator (a `DB:Admin` member
  too) makes a free table of every `CREATE TABLE` not said `IN DATABASE`,
  so a primary key there is 5132, and with `IN DATABASE` it gets its
  rights on the table before the key's objects (`fixtures/sql/ddadminpk`;
  for DBF tables `fixtures/sql/ddcreatedbf`). A rename through `AdsDDRenameObject` needs the
  `ALTER` right of the object, checked after the names
  (`fixtures/sql/ddfuncrights`); the grant option's refusal names the
  object for functions, packages, links, users and groups
  (`fixtures/sql/ddgrantopt2`). The other `AdsDD` calls by
  such a user answer 5054 and 5132 themselves (`fixtures/sql/ddapirights`):
  the property setters need `ALTER` on the object (on the table for a
  field or an index, on the publication for an article),
  `AdsDDSetDatabaseProperty` the administrator or `DB:Admin`,
  `AdsDDDeleteIndex`, `…RemoveIndexFile`, `…DeleteArticle` and
  `…AddUserToGroup`/`…RemoveUserFromGroup` `ALTER` (on the table, the
  publication or the group), `…RemoveRefIntegrity` `ALTER` on both tables
  of the rule, and the removals of a table, view, procedure, user, group
  and publication `DROP`; a name that is not found is refused before the
  right. A rename through `AdsDDRenameObject` needs `ALTER` on a user or a
  publication, on the table of a trigger, and on both tables of an
  integrity rule (and is then 5138 for all but the administrator and
  `DB:Admin`; `fixtures/sql/ddrenamerights`). `AdsDDRemoveTrigger` finds a trigger
  by `table::trigger` only (a bare name is 5132), `AdsDDAddIndexFile` of a
  missing file is 7041 after `ALTER` on the table, a property id the
  original does not know is 5126 (5042 for a user group) before the name
  and the right, and `AdsDDSetSubscriptionProperty` and
  `AdsDDDeleteSubscription` need `ALTER`, and `DROP` on the subscription
  and on its queue table (`fixtures/sql/ddapirights2`). Not matched:
  `AdsDDAddIndexFile` of an existing file that is not an index (7010 in
  the original, 5088 here), an id the original knows but tflip does not
  write (`AE_NOT_IMPLEMENTED`).
- **Functions and packages made with SQL** (`CREATE`, `ALTER`, `DROP
  FUNCTION`, `CREATE`, `DROP PACKAGE`) are written as the original writes
  them, the dictionary byte for byte (`fixtures/sql/ddfuncs`), with its
  refusals and the rights a user who is not the administrator needs. A
  rename by such a user checks the right first through `ALTER FUNCTION`
  and the names first through `AdsDDRenameObject`, and rights held through
  a group work (`fixtures/sql/ddfuncrights`, `ddfuncgroup`).
- **Triggers made with SQL** (`CREATE`, `ALTER` and `DROP TRIGGER`, on a
  table or `ON DATABASE`) behave as the original's
  (`fixtures/sql/triggers.json`, `trigdb.json`), their `__error` table too
  (`trigerr.json`), and a failing `CONNECT` trigger refuses the login with
  7109 and a failing `OPEN_TABLE` one the open (`trigdbfire.json`,
  `trigdbopen.json`), but: only a script body is taken (not `FUNCTION … IN
  LIBRARY`); the dictionary record is written as the original writes it
  (byte for byte for database triggers and `ALTER TRIGGER`,
  `fixtures/sql/trigdb/`); a script too long for the record's property
  field goes on in `More Property` split where tflip reads it, not compared
  with the original's split. `OPEN_TABLE` and `CLOSE_TABLE` triggers fire
  as the original's (`fixtures/sql/trigdblog.json`, `trigdblog2.json`,
  `trigdblog3.json`: each table a statement opens once, closed when the
  statement runs other SQL, closes or its connection does; the open count
  across connections; a client's open of a cursor), but: a statement closes
  several tables in the order it opened them (a `SELECT` with no condition
  in reverse), where the original's follows its plan (a three-table `JOIN
  ON` closes t3, t1, t2); the tables a table trigger's statements open close
  at other moments; and a procedure's cursor holds its table to the end of
  the procedure, where the original's closes at `CLOSE`. `__info` of
  `CONNECT` and `DISCONNECT` differs in `OpCount` (tflip's is 27, or 28 for
  a user other than ADSSYS, plus its statements; the original's counts the
  requests it served), and `ClientName` and `LoginName` are the local
  server's (not probed on a remote one). `CONFLICT` triggers are kept but never fire, as
  tflip has no replication to fire them. A script statement's error is
  located in the script (`** Script error information`) only when it says
  nowhere else where it is.
- **Access rights in SQL**: an `INSERT`, `UPDATE` or `DELETE` the user may
  not do fails with 7087, and column rights and `TABLE_PERMISSION_LEVEL`
  are enforced as the original's (`fixtures/sql/ddcolrights`), and a user
  with `INSERT`, `UPDATE` or `DELETE` but no `SELECT` changes the table as
  the original lets it (`fixtures/sql/ddinsertonly`); an `INSERT …
  SELECT`, an `INSERT … VALUES` with a subquery and an `UPDATE … SET` with
  a subquery that read the table they change are 7079 for a user holding
  the right to change it but not to read it (7087 for one with no right);
  through the table API a set on a record that was there needs `UPDATE`
  and the columns the user may update (5171 for the others), and a set on
  a record just appended (not yet written) and deleting it need `INSERT`
  (`fixtures/sql/ddinsertonly2`). A `DELETE` (and `UPDATE`) whose
  condition names a column of its own table is 5054 "Read permission not
  granted to table" for a user who may not read it, one that reads the
  table in a subquery is 7079 (7087 for a user with no rights), one that
  names none (`WHERE 1 = 0`, a subquery over other tables) needs no read
  right, and a column the user may not read in a subquery's select list in
  a condition is 5171 (`fixtures/sql/ddinsertonly3`). After a
  refused connect the original keeps the 7109 or 7078 text through a
  later failing call, where tflip's disconnect resets it to 5018.
- **Free tables and a dictionary table's missing files**
  (`fixtures/dictionary/freetables.json`): `CheckFreeTableAccess` refuses
  free tables (7087) to a user who is neither ADSSYS nor of `DB:Admin`, the
  rights properties of users and groups cannot be set (5126, 5042), a user
  without `ALTER` makes temporary indexes only, and `Table_Auto_Create`
  makes a missing `.adt`, `.adi` or `.adm` again, as the original does.
  But: `AdsCreateIndex` into the table's own index file on a table open
  shared is 5075 in the original for every ADT table, in tflip for
  dictionary tables only; auto-creating `.cdx` and `.dbf` dictionary tables
  is untested; the original's 5168 in its error log for an auto-created
  table is not written (tflip keeps no error log); the free table a user
  without the right to create tables makes was not probed with
  `CheckFreeTableAccess` or `Check_Free_Table_Access`.
- **The server settings `Check_Free_Table_Access` and
  `Disable_Free_Connections`** (`tflipd --check-free-table-access`,
  `--disable-free-connections`; `fixtures/events/freeconf_*.json`) are the
  original's, ADSSYS bound too, but: a procedure that reads a free table
  under `Check_Free_Table_Access` fails with 5154 wrapping 7087 as in the
  original, but its text ends " ** Script error information: ... Error in
  stored procedure: p1" where the original's has " Error executing stored
  procedure: p1" (`events::server_settings::check_free_table_access_in_a_procedure`,
  ignored); `AdsCreateSQLStatement` on the handle 0 of a refused free
  connection is 5024 where the original's last error is 5018 "... The
  given handle is not readable."; and the settings are `tflipd` flags, not
  a configuration file.

## Opening tables

- **Collation mismatch options** (`ADS_IGNORE_COLLATION_MISMATCH`,
  `ADS_REINDEX_ON_COLLATION_MISMATCH`) are the original's
  (`fixtures/collation/mismatch.json`), but: with both a custom order and
  another user's open, which refusal comes first is not known (tflip gives
  the custom order's).
- **A table opened as another type**: a DBF table opened as ADT fails with
  5081 (the original: 7010 or 7016), an ADT table opened as a DBF type with
  5081 (7014), and a Visual FoxPro table opened as NTX with 5025, where the
  original opens it (`fixtures/config/connect101.json`; the replay checks
  only that the open fails).
- **Table listings**: `AdsFindNextTable` with an unknown listing handle
  fails with `AE_INVALID_HANDLE`; the original returns `AE_SUCCESS` without
  a name.

- **DBF orders** (`.ntx`, `.cdx`, `.idx`; a structural `.cdx` opens with
  its FoxPro or VFP table) walk, count, number and seek their keys as the
  original engine does (`fixtures/dbf/indexes.json`), but:
  - keys are built from records as the original builds them (checked
    against every order of the fixtures, VFP TimeStamp keys against
    `fixtures/write/vfp_timestamps`, expressions whose value is a timestamp
    against `fixtures/write/cdx_timestamps`). But a `CREATETIMESTAMP` of no
    date or time (month 13, February 30, hour 25, negative or 1,000
    milliseconds and more) keys as the empty timestamp's date with the time
    added; the original refuses to write the record, and to build its key,
    with `4012` *Invalid date evaluated in index key expression.* (day 0, as
    in `CREATETIMESTAMP(2020, 1, 0, …)`, it keys as the empty timestamp). A
    candidate order refuses a record whose key is NULL with `5208` *The
    index key cannot be null.*, as the original does (a VFP primary key, in
    SQL too: `fixtures/sql/dbfpk.json`). Finding a record in an NTX or
    CDX order walks it. Off a record `AdsExtractKey` gives the first or
    last key; after a seek that ends at EOF the original gives another key,
    of the place its search stopped.
  - text that is not a date in the date format seeks a date or timestamp
    order as the empty date (`00000000` in NTX), or fails with
    `AE_INVALID_DATE`; the original reads it leniently (`1.5` as a date
    in 1900).
  - a number too wide for an NTX numeric order's key seeks as asterisks;
    the original lands elsewhere.
  - a VFP collation (`GERMAN_VFP_CI_AS_1252`) is not checked when the
    table opens, and seeks in its orders compare the sought bytes with
    its weights.
  - `AdsGetIndexFlags` of CDX orders is not probed: xBase `UNIQUE` is
    `ADS_UNIQUE` and a candidate `ADS_CANDIDATE`.
- **Encrypted Clipper, FoxPro and Visual FoxPro tables** (free:
  `AdsEnableEncryption`, `AdsEncryptTable`, `AdsDecryptTable`,
  `AdsEncryptRecord`; in an RC4 dictionary: `ENCRYPT_NEW_TABLE`,
  `AdsDDAddTable`, `sp_EncryptTable`) are read, written, packed and
  restructured as the original does, byte for byte
  (`fixtures/write/dbf_encryption_*`, `fixtures/aes/rc4tp` y1 to y3, y6
  to y9), in a transaction too (`fixtures/write/dbf_encryption_tx`), and
  memo values are read at their record's number for SQL conditions (y10).
  Full-text orders over their memo fields (`CREATE INDEX … CONTENT` on a
  FoxPro or VFP table of an RC4 dictionary, recorded in the dictionary as
  the original does, and `CONTAINS` after writes; y11, y12), an ADS
  VarChar's remainder in the memo file (in the clear) and Binary and Image
  values (a SIx tag in the clear, the data one stream; y13, y14) are the
  original's, and so is the record a handle stands on after
  `AdsDisableEncryption` (it reads as it was loaded, deciphered, until the
  handle moves; `fixtures/write/dbf_encryption_cached_*`). A change or
  delete of that record after `AdsDisableEncryption` is refused (5104)
  whether one record or the whole table is encrypted, and writes nothing
  (ADT included). But on an ADT table a record read enciphered after the
  password is given again has a memo reference the original fails with
  5001 ("Not enough memory for memo field m1"), where tflip reads it as
  NULL; and that an exclusive handle keeps its record where a shared one
  reads the file again is inferred from two scenarios, not probed on one
  table.
- **VFP tables laid out for AES** (an AES dictionary's, and a free one
  made on a connection with `EncryptionType=AES128`/`AES256`) are made,
  armed, read, written, packed, restructured, encrypted and decrypted as
  the original does, compared deciphered (`fixtures/aes/rc4tp` v1 to v12).
  Reads, copies, appends, writes and restructures without the password
  match the original (`fixtures/write/dbf_aes_nopassword`,
  `fixtures/aes/rc4tp` v13 to v16), and so do copies, which the original
  writes laid out for AES but in the clear (see ADS_BUGS). But:
  `sp_EncryptTable` of a plain VFP table makes new Field objects in the
  original, so the dictionary's next object id and `.ai` differ;
  transactions on such a table are not probed (the local engine rolls
  nothing back, and the remote server cannot make an AES dictionary: 5214,
  no OpenSSL), so tflip uses the RC4 DBF handling with AES enciphering,
  checked only for the rows a rollback and a commit leave; the copy of an
  armed free AES ADT table without its password is laid out as the
  original's, but its contents and memo file differ (v14); `AdsConvertTable`
  and `sp_CopyTable` of an AES table are not probed; `SELECT … INTO` with a
  Clipper or FoxPro statement table type still makes an ADT table (only VFP
  was probed: it makes a VFP table); and a Clipper or FoxPro table on an
  AES connection is not probed beyond being laid out plain.
- **Unreadable index files**: a table whose `.adi` is encrypted or cannot
  be read opens without orders instead of failing.

## Fields

- **`AdsSetBinary` in pieces** (ADT Binary, Image and VarBinary;
  `fixtures/write/binary_pieces*`) pads a longer total with zeros and cuts a
  shorter one, where the original keeps the bytes it cut: a value of 10
  bytes cut to 4 and given a total of 10 again has its old bytes 5 to 10
  back (also after a write, and per record), and its `.adm` block holds
  them (see ADS_BUGS). Pieces of DBF memo and binary fields, and of
  statement parameters, are not probed.
- **`AdsGetBinary` into a short buffer** sets `*len` to the bytes copied,
  where the help reads as giving the length stored (not probed).
- **Clipboard (non-Windows build only)**: `AdsImageToClipboard` copies an Image
  field's BMP to the Windows clipboard as `CF_DIB` when tflip's `ace32.dll` runs
  inside a Windows process — byte-for-byte the format the original engine sets,
  with Windows synthesising `CF_BITMAP`/`CF_DIBV5` (verified against the live
  engine; see `tools/ace/clipboard`). Built for a non-Windows host (tflipd on
  Linux) there is no clipboard, so a valid image reports `AE_NOT_IMPLEMENTED`;
  an invalid one is still rejected (5091/5001) and a non-Image field fails as the
  original does.
- **A decimal SQL number too long for a Numeric** and without an exponent,
  `1000000000000000000000000000000.0`, is a Double in the original, read
  inexactly as a number with an exponent is (see the changelog); tflip keeps
  it an exact Numeric.
- **Code pages**: 8-bit text passes through as bytes in the client's ANSI
  code page, and is read in it for the Unicode calls, as in the original
  engine (probed under code pages 1252 and 1251; see
  [PLATFORMS.md](PLATFORMS.md#character-encoding)). But:
  - A table whose ADI indexes were built under another ANSI (static)
    collation than the connection's is refused with
    `AE_INDEX_COLLATION_MISMATCH` (5175), as in the original engine — an ADI
    records a CRC-32 of its collation's weight table in its header, which tflip
    compares with the connection's on open (see the changelog and Collations).
  - Text given through the Unicode calls (`AdsPrepareSQLW`, `AdsSetAOF100`
    with UTF-16, `AdsSetStringW`, ...) becomes the client's bytes, as in the
    original engine, so it cannot name a Unicode field's characters the
    code page lacks.
  - Expression `UPPER()`/`LOWER()` fold case by the static collation, and
    expression string *comparisons* (`=`, `<`, ranges) now weigh bytes by it
    too (as the original does), and so do the SQL evaluator's (`WHERE`,
    `ORDER BY`, `DISTINCT`, `GROUP BY`, `UNION`, `MIN`/`MAX`); SQL
    `UPPER`/`LOWER` return the collation's case. See Collations.
  - OEM tables (Clipper and FoxPro tables opened with `ADS_OEM`): their
    8-bit text is converted to ANSI or Unicode for the getters, as in the
    original engine, under OEM code page 437 with the original's own
    tables, under others as Windows converts them (not probed). Text a
    client gives is converted to OEM (best fit for ANSI text, as for
    writes; code page 437 exactly for Unicode text) in seeks, scopes,
    filters, AOFs, expressions, `AdsLocate` and index expressions and
    conditions (`fixtures/expr/oem.json`), and in the literals and text
    parameters of SQL statements whose tables are all OEM
    (`fixtures/sql/oem.json`); a statement mixing OEM and other tables
    (not probed) converts none, nor does a literal in a subquery over
    `system.iota` inside one, and a remote client's cursor does not carry
    the OEM flag yet. What a script, a subquery, a derived table or a
    variable reads out of OEM tables is ANSI text, as the original's
    (`fixtures/sql/oemrows.json`), and `ASCII` of OEM text gives the ANSI
    code; a view over an OEM table is not probed. A
    Unicode character the code page lacks matches nothing in the original;
    tflip turns it into NUL, so it equals `CHR(0)`. `AdsGetFilter` after a
    filter set as UTF-8 or UTF-16 gives the original UTF-8 text; tflip's is
    ANSI.
    A table made as OEM has no language driver in its header, as the
    original's (`fixtures/sql/ddbf`), and one a dictionary has as OEM is read
    as one (`fixtures/dictionary/dbfcorners`).
    (Their NTX and CDX keys are now ordered by the `OEM_CHAR_SET` collation,
    from `extend.chr`, as the original does; see Collations.)
  - `AdsSetStringFromCodePage` converts from the single-byte code pages the
    original accepts (437, 500, 737, 775, 850–869, 874/875, 1026, 1250–1258)
    and the four double-byte pages (932 Shift-JIS, 936 GBK, 949 UHC, 950 Big5),
    with the engine's own tables (it converts through its bundled ICU, probed
    byte-pair by byte-pair); a page it has no converter for fails as the
    original does (1565).
  - `AdsGetIntProperty` reports code page 1252 for every table, as the
    original did on a US Windows client.

## Seeking, scopes and index orders

- **Bookmarks** are the original's in length and bytes (8 through a table,
  12 and the key's length through an order, whose handle word is tflip's
  own handle), and so are `AdsCompareBookmarks` and `AdsGotoBookmark60`
  (`fixtures/bookmarks/bookmarks.json`), but: at BOF and EOF the key in an
  order's bookmark is that of the record the original has in its buffer,
  which depends on what was read before; tflip gives the table's last
  record's key at EOF and the order's first key at BOF.
- **Unicode key options**: `ADS_UCHAR_KEY_SHORT`, `_LONG` and `_XLONG` are
  the original's for ADI orders (`fixtures/write/uchar_keys`), not probed
  for DBF index types; `AdsBuildRawKey100`'s `ADS_GET_PRIMARY_WEIGHT_LENGTH`
  cuts the key of an order's first segment only (5042 for a later NChar or
  NVarChar segment, as in the original).
- **NChar and NVarChar orders**: their keys are ICU sort keys (the root
  collation, `:en_US`, of ICU 4.0), which tflip builds as the original
  engine does, for seeks, scopes, `AdsExtractKey` and writes (append,
  change, delete, pack and reindex of tables with such orders, alone or
  among several fields, byte for byte as the original, `fixtures/write/
  nchar_indexes`), and creates (`fixtures/write/nchar_create`). The weights
  are `crates/tflip-storage/src/icu_root.txt` (all BMP characters, plane 1,
  the contractions of UCA 5.1.0, worked out black-box by
  `tools/ace/seek/nkeys.py`) and formulas for ideographs, unassigned
  characters (ICU's implicit weights), Hangul syllables and the Thai and Lao
  vowels before consonants; ~50 characters whose elements could not be
  worked out (`?` in the table) fail with `AE_NOT_IMPLEMENTED`, writes of
  such text before anything changes. Orders on an expression over a Unicode
  field are the original's (`fixtures/write/nchar_expr`: `UPPER`, `LOWER`,
  the trims and pads, `SUBSTR`, `LEFT`, `RIGHT`, `REVERSE`, `IIF`,
  `COALESCE`, `+`, `LEN`; `fixtures/write/nchar_expr_more`: `REPEAT` with
  a constant count, `DESCEND`, `VAL`, `SOUNDEX`, `BASE64ENCODE`, `COALESCE`
  with an 8-bit first argument; keys 3 bytes a character and 6 long, the width
  worked out by the original's rules, `UPPER` and `LOWER` mapping a
  character at a time by Unicode 5.1, `icu_case.txt`), but: one with
  `TRANSFORM`, `AT`, `REPEAT` with a count that is not a constant (the
  original fails writing it with 4004), a non-text `IIF` or `COALESCE`
  branch, or `DESCEND(nc) + nv` (the original refuses it with 3009) fails
  with 5042 where the original builds it; an `IIF` with an 8-bit first
  branch fails with 5042 (the original's keys of it hold stale bytes);
  `REPEAT` of a NULL NChar value, written to an order, fails with 4004 in
  the original and gets a zero key in tflip; `COALESCE(NULL, nc)` is
  refused with 3009 and `NULL + nc` with 4009 by the original, and tflip
  builds them (the same for `COALESCE(NULL, c)` and `NULL + c` over 8-bit
  text); `BASE64ENCODE` of an empty NVarChar value leaves the previous
  key's bytes in the original's key, where tflip writes the empty
  encoding; the original's `SOUNDEX` takes a space as a separator, which
  tflip skips; the original gives `DESCEND(c)` the pad byte 0xE0 and a
  constant expression such as `REPEAT('é', 3)` its last byte (0xE9), where
  tflip writes 0x20; a non-constant count (`LEFT(nc, id)`) leaves
  stale bytes after the original's keys where tflip writes zeros; a `FOR`
  condition comparing Unicode text reads it as lossy 8-bit text; a numeric
  expression order (`LEN(nc)`) sought with text differs (the original found
  1 for `É`, tflip 0); and a duplicate in `AdsCreateIndex` of a unique
  Unicode order answers 7200 where the original answers 7059. Every such
  order is taken to be `:en_US`: the files of the original keep that name in
  the tag header (and four bytes after it), which tflip writes and keeps.
- **An order made after `AdsSetCollation`** on a free ADT table by
  `AdsCreateIndex61` came out in binary order in one probe, where the
  original's is collated; whether the collation files were found there
  is not known.
- **Collations**: keys are weighed as the original engine weighs them,
  from its collation files (`ansi.chr`, `adscollate.adt`) next to the DLL
  (or in `TFLIP_COLLATION_DIR`; for `tflipd`, `--collations`), which tflip
  does not ship: the static ANSI collation `ANSI_CHAR_SET` of the
  `adslocal.cfg` there (and the OEM collation `OEM_CHAR_SET`, from
  `extend.chr`, an OEM Clipper or FoxPro table orders its keys by), and the
  dynamic collations orders name (a table whose orders have one opens only
  with it, `AE_COLLATIONS_DO_NOT_MATCH`, as in the original). Without the
  files, dynamic orders cannot be built and the static collation is the one
  below that an empty `ANSI_CHAR_SET` gets (it needs no files).
  Expression `UPPER()`/`LOWER()` fold case by the static collation, as the
  original does (`UPPER(b)` is `weights[upper[b]]`), and expression string
  *comparisons* (`=`, `<`, ranges) weigh their bytes by it too — a
  case-insensitive operand folding case through the collation first — so, for
  example, `ä` sorts just after `a` under German; the SQL evaluator's
  comparisons and `UPPER`/`LOWER` still work on bytes, and `==`/`$` stay exact
  byte operators (but fold CICHARACTER text by the collation). A CDX order's
  keys of numbers and dates are not weighed, as in the original
  (`fixtures/write/aofs_static_keys`). An ADI index
  built under another static collation is refused on open
  (`AE_INDEX_COLLATION_MISMATCH`, error 5175), as the original engine does,
  by comparing the CRC-32 of the collation's weight table the ADI records in
  its header with the connection's. With `ANSI_CHAR_SET` empty the static
  collation follows the host's user locale, as in the original: the bytes
  themselves under US, UK and Canadian English, else Windows' sorting of the
  256 bytes under that locale. On Windows tflip asks Windows; elsewhere it
  maps `LC_ALL`/`LC_COLLATE`/`LANG` to a Windows locale and uses tables
  derived under Wine, which match native Windows 10 for the locales probed
  there (German, Swedish) but could differ from another Windows version's
  sorting in a character or two. A POSIX locale Windows has no counterpart
  for is taken as its language's main locale, or else as `en-US`. A text sought
  in a dynamic order is compared as its bytes, and so not found, where the
  original weighs it by the index's collation (`AdsSeek` of `ä` in the
  Czech order of `fixtures/adt/collation_dynamic` finds it;
  `fixtures/collation/mismatch.json`).
- **Date and timestamp texts**: a Date value (setters, seek keys, `CTOD`)
  is read from text as the original reads it (`fixtures/fields/dates.tsv`),
  in formats with separators and without, and so is a timestamp's text
  (`fixtures/fields/tstexts.tsv`), as a TimeStamp value, a seek key in an
  ADT order on a TimeStamp field and `CTOTS`'s argument. What is left: a
  failed TimeStamp set leaves the field as it was, where the original
  sometimes writes part of it (see `docs/ADS_BUGS.md`); a seek key with no
  date is sought as the day 0 at midnight, which the probe can only place
  between the empty value and the first day (no record can hold the day 0
  with a time); and a VFP TimeStamp order's seek key reads its text the same
  way unprobed.

## Moving

- **Going past the last record**: `AdsGotoRecord` to any slot past the last
  record leaves the cursor at EOF (BOF and EOF at once). tflip always returns
  `AE_SUCCESS` there; the original returns `AE_SUCCESS` too — *except* when the
  cursor's current record is a deleted record, when it returns
  `AE_INVALID_RECORD_NUMBER` while still moving to EOF (worked out on real
  Windows; see [`docs/ADS_BUGS.md`](ADS_BUGS.md)). The
  `fixtures/write/goto_deleted` golden shows this: its `goto 5` fails `5022`
  because the preceding `goto 2` left the cursor on the deleted record. tflip
  does not reproduce that inherited error.
- **A deleted record the cursor was left on** (by `AdsGotoRecord`, 5022)
  is the original's (`fixtures/write/deleted_cursor`), but: the count
  `AdsGetRecordCount` gives one lower until an append is kept per handle,
  and whether the original's other handles see it is not probed; a single
  recall in a table with memo fields is not probed.
- **A too long VarChar set on a DBF table** (`set v string` of a GUID's
  text on an NTX table): the original writes memo-pointer bytes into the
  record file at once, and `AdsCancelUpdate` keeps them; tflip leaves the
  record as it was (left out of `fixtures/write/set_errors`).
- **Triggers' savepoints** are the original's (`fixtures/sql/txtrig2.json`:
  an `AFTER INSERT` trigger's rollback to its own savepoint takes the insert
  back; a `NO TRANSACTION` trigger is a call, as a procedure is), but: two
  rollbacks to savepoints in one trigger keep the insert in the original,
  where tflip takes it back; a `NO TRANSACTION` trigger outside any
  transaction may set savepoints in the original (as if it had a
  transaction of its own), and gets 5047 here; a statement that fails in
  the caller's transaction through such a trigger takes the trigger's
  writes back there, where tflip keeps them; and the log on disk does not
  say a record was taken back, so a crash between a commit's entry and its
  applying could commit it. 7130 and 7187 were not reproduced, with nested
  triggers, procedures, scripts or unended transactions either
  (`fixtures/events/events_tx_sub.json`, see
  [TRANSACTIONS.md](TRANSACTIONS.md)). A transaction's record lock belongs
  to the handle that wrote it: a second handle of the same connection
  cannot lock, write or unlock it (5035, 5062), as in the original
  (`fixtures/write/tx_handles`); SQL statements of the connection still
  lock it.
- **Triggers and open tables**: the original goes on with the triggers a
  table had when a statement opened it until the statement closes, so a
  `CREATE` or `DROP TRIGGER` on the same statement handle does not apply to
  its next statement; tflip loads them for each statement
  (`tools/ace/sql/txtrig2.txt`). Across connections the original has one
  open instance of a table, so a trigger created while one connection's
  statement holds the table fires for no connection until none has it open
  (a connection keeps a table its statements closed until it disconnects),
  and one dropped stops at once on the remote server
  (`fixtures/events/events_trigger_open.json`). A `tflipd` connection reads
  the dictionary again when its files changed (time and size), so another
  connection's `CREATE TABLE`, `CREATE TRIGGER` and the like are seen; a
  change in the same clock tick as the last read can be missed.
- **Events** (`sp_CreateEvent`, `sp_SignalEvent`, `sp_WaitForEvent`,
  `sp_WaitForAnyEvent`, `sp_DropEvent`, `sp_DropAllEvents`) run as the
  original's remote server runs them (`fixtures/events/`), shared by the
  connections of the process and the sessions of `tflipd`; but not by the
  processes of a local server; a procedure's argument `(SELECT n FROM
  __input)` is 2200 (a variable set from it works). `sp_CancelQuery` finds
  a wait of another connection, as `sp_GetSQLStatements` lists every
  client's statements on the same dictionary or folder
  (`fixtures/events/events_statements.json`). How long a wait lasts differs on purpose (see
  DIFFERENCES).
- **A record recalled in the transaction that deleted it** is undeleted
  cleanly; the original's commit leaves its slot on the free list as well
  (ADS_BUGS, `fixtures/write/tx_recall`), so the files differ there.
- **A rolled-back append**: going to a record a transaction appended and
  rolled back leaves the original's handle at EOF; tflip's goes to the
  deleted record as to any other (`fixtures/write/tx_basic`).

## Moving in index orders

tflip builds a record's key in an order as the original engine does
(`AdsExtractKey`; checked against every key of the fixtures), and finds
the current record in an order by seeking its key, but:

- **Orders whose keys tflip cannot build** (binary orders, dynamic
  collations without the collation files, NChar and NVarChar text with one of
  the ~50 characters it could not work the ICU weights of out) are still searched from
  their first key for the current record, and `AdsExtractKey` fails on them
  with `AE_NOT_IMPLEMENTED`.
- **`AdsExtractKey` at BOF** gives an empty record's key; the original
  engine gives the key of the record it was on before.
- **Skipping from a record the order has no key for** (one a `FOR`
  condition leaves out, or a deleted ADT record) goes as the original's does
  (`fixtures/write/skip_unkeyed`; see ADS_BUGS.md), but with a filter or a
  scope set, which was not probed: the first step then goes to the next
  visible key in the scope. In an order without keys, it lands at EOF only
  (forward) or BOF only (back), where tflip sets both.

- **`AdsSkipUnique` back from the first key**: tflip goes to BOF, as the
  original engine does in most orders; but in an order whose first key is
  NULL (a Logical order of `fixtures/adt/adi_keys`), the original stays on
  that key's record.

## Relations

`AdsSetRelation`, `AdsSetScopedRelation` and `AdsClearRelation` work as
the original's local and remote servers have them (`fixtures/write/
relations` and `relations_dbf`): the child follows at its next use, a
parent off its records sends it to EOF, scoped relations scope the child's
order, cycles fail with 5077. But:

- **A grandchild whose parent found no key**: the original's first call on
  it fails with 5068 (`AdsGetRecordNum` in the probe) before it reports
  EOF; tflip's goes to EOF without the error.
- **Negative numbers into an NTX order** are not found, as in the
  original; that it seeks them as plain text is inferred, not probed byte
  for byte.
- **A child with two parents** (two tables relating to it): which parent
  it follows when both moved was not worked out; tflip follows the one
  that moved last.

## Filters

AOFs (`AdsSetAOF`) and record filters (`AdsSetFilter`) hide the records
they fail, as in the original engine (`fixtures/expr/filters.json`
replays the probed behaviour). AOFs are optimized as the original engine
optimizes them, with the index orders open on the table: `AdsGetAOFOptLevel`
and `AdsEvalAOF` report its levels and its text of the part not optimized,
and AOFs show, keep and count records as its do, fixed, customized and
refreshed ones among them, while two handles write
(`fixtures/write/aofs`). But:

- **Records found by the orders**: every part the orders serve is looked
  up in them (`fixtures/write/aofs_lookup`, `aofs_more`, `aofs_static`,
  `aofs_static_keys`, `aofs_rest`), but keys of several fields of a CDX
  order beyond a leading part, expressions of several segments, and
  CICHARACTER text of an expression order or a key of several fields under a
  static collation, which are evaluated on every record when the AOF is set.
  Where the original's orders find other records than evaluating does (`$`
  by the keys' bytes, `ISNULL()` through a FoxPro order, `EMPTY() = .F.`
  through a Visual FoxPro order, `=` with text longer than a leading part of
  a FoxPro key; see `docs/ADS_BUGS.md`), tflip shows the orders' records, as
  the original does; written later, such records are evaluated the way the
  order found them. How the original evaluates those was not probed. Under
  a static collation, `$` on CICHARACTER text is evaluated, folding both
  sides by the collation: the original's order compares the folded key with
  the value as it is, which differs for a value with capitals (not probed
  under a static collation).
- **`.NOT.` before a comparison of `EMPTY()` with a logical value**, written
  without parentheses (`.NOT. EMPTY(name) = .F.`): tflip takes it as one
  term, not optimized; the original optimizes it as `.NOT. (EMPTY(name) =
  .F.)` (fully in a FoxPro table, partly in an ADT table), with the same
  records.
- **`YEAR()` terms** an order finds are evaluated, once the AOF is set, as
  the order finds them (the year of the value's whole part; see
  `docs/ADS_BUGS.md`), also for records written later; how the original
  evaluates those was not probed.
- **Dynamic collations**: an ADT table opened with one compares text in its
  expressions by the collation's weights (`=`, `<>`, `<`, ...; not `==` or
  `$`), probed for AOFs (`fixtures/write/aofs_collation`); SQL compares
  such text by the statement's dynamic collation (see [SQL](#sql)), and DBF
  tables with a collation compare as before. Moves pass the records the AOF found by
  without reading them only until the table is written in the process;
  then they evaluate each record. A live SQL cursor's `WHERE` is such an
  AOF too (see [SQL](#sql)).
- **The part not optimized** of a partly optimized expression is printed by
  rules learnt from the probes (`tflip_expr::optimize`); shapes they did
  not cover may print otherwise. Orders on `DELETED()`, which the original
  engine uses to leave deleted records out, are not used.
- **Other processes' writes**: an AOF follows the writes of every handle in
  the process, as the original's does; a record another process changes is
  evaluated as moves reach it, unless the AOF found it out when it was set
  and the table has not been written in the process since. The original
  keeps what it had for such records.
- **Customizing** an AOF that is resolved as moves reach records and not
  fully optimized evaluates again, when moves reach them, the candidates
  it had evaluated before; the original keeps what they were then.
- **Not probed**: `AdsGetKeyNum` and `AdsGetKeyCount` with
  `ADS_RESPECTSCOPES` respect filters too; `AdsLocate` goes to EOF (BOF
  backwards) when no record matches; `AdsSetFilter` keeps its text as
  given.

## Expressions

The expression engine (`tflip-expr`) gives the original's results on the
probed corpus (`fixtures/expr/expressions.json`), but for:

- **NULL results** through `AdsEvalNumericExpr`: the original returns
  whatever number it left in memory, tflip 0. Through
  `AdsEvalStringExpr`, both return zero bytes, but not always as many.
- **NChar and NVarChar**: the original evaluates them to ICU sort keys
  (`AdsEvalStringExpr` returns those), tflip to their text in
  Windows-1252, and compares them as bytes.
- **Collations**: SQL strings compare by the static ANSI collation's
  weights (CICHARACTER text by its lower case's), padded with spaces, as
  probed (`fixtures/sql/collation_compare.json`); OEM tables' text too, as
  SQL knows only the connection's collation (not probed). Under a dynamic
  collation (`AdsStmtSetTableCollation`, else `AdsSetCollation`) all of a
  statement's text compares by it, as probed
  (`fixtures/sql/collation_dynamic.json`, the four ways of
  `docs/ADS_BUGS.md`), but for two literals, which the original compares
  character by character, unexpanded (`'äa' > 'aeb'`), and tflip as it
  compares columns; which of a group's equal values (`apfel`, `Apfel`)
  `GROUP BY`, `DISTINCT` and `UNION` show (tflip the first); `DISTINCT` on a
  column a dynamic-collation order serves, which the original sorts by the
  order's keys; and `LIKE`, which folds case under a collation that ignores
  it but was probed only for that. Not probed: DML, DBF tables, and the
  dictionary connection's collations. `COLLATE()` fails with 3424 (or 3524)
  for any collation.
- **`STR()` of computed numbers** (`STR(k_numeric + 1)`): the original
  picks the decimals by a rule the probes did not make clear (`2.5`, but
  `2.500000000` for a Double field plus 1); tflip uses the larger of the
  operands' decimals, at most 9 (2 for a division).
- **Functions not done**: `TRANSFORM()` of a number with a picture of
  other characters than `9`, `.` and the literals `%`, `-`, `/`, `:`, or
  other options than `B` and `R` (`,`, `$`, `#`, `@C`, `@Z`, `@X`, `@(`),
  which gives the number as plain digits; an index key of a `B` picture is
  as long as the picture, where the original's is as long as the value at
  the order's creation; `TTOC()` and `SYS()` fail as unsupported functions; `LOWERW()`
  and `UPPERW()`, which the help lists among the index functions, are
  unknown to tflip; `CONTAINS()`
  holds for any value that is not NULL, as the original does without a
  full-text index. `NEWIDSTRING(format)` is done — a GUID in the format the
  string names (`D`/`DELIMITED`, `B`/`BRACKETED` in square brackets,
  `P`/`PARENTHESIS`, `N`/`NUMBERS`, `M` MIME Base64, `F`/`FILE` URL-safe
  Base64), matched to the engine on the VM; the documented `MIME`/`REGISTRY`
  long names are rejected by the engine itself (see `docs/ADS_BUGS.md`), and the
  GUID value, being random, is matched by shape, not byte for byte.
- **Errors**: as in the original engine, the first error from the left is
  reported (an unknown name before two operands in a row or a missing
  parenthesis), but for a few texts: `UPPER(c_char` is 3118 in tflip, 3108
  in the original; `foo.c_integer` 3110, not 3111; `.AND. foo` 3111, not
  3101.

## SQL

What tflip runs, and what the probes (`fixtures/sql/statements.json`) show
it does otherwise:

- **SQL on DBF tables**: a statement opens and creates its tables as the
  type `AdsStmtSetTableType` gives, and reads, writes, creates, alters and
  drops Clipper, FoxPro and VFP tables and their orders as the original
  engine does (`fixtures/sql/dbf.json` for what it answers,
  `fixtures/write/sql_dbf` for the files, byte for byte). But:
  - **Empty and blank values** are as the original has them
    (`fixtures/sql/dbf.json`; `docs/ADS_BUGS.md` describes them): a Clipper or
    FoxPro table's empty field is NULL to `IS NULL`, `IFNULL` and `COALESCE`
    and its value to the rest, a blank one shows as NULL and groups apart, and
    a VFP table's blank date is 1899-12-30. But for a VFP table, `IFNULL`
    of an empty value (0, false, spaces) gives the value where the original
    gives the fallback (listed as known in the replay), and its blank date is
    1899-12-30 throughout, where the original's `YEAR()` of it is NULL and a
    live `WHERE d = '1899-12-30'` misses it; a blank VFP timestamp stays the
    empty one (the original shows 1899-12-30 00:00 in a static cursor, yet
    `MIN` of it is NULL). Not probed further: `d + 1` of a Clipper blank date
    (the original fails with 5095), and a `{static}` `SELECT` in `FROM`, whose
    blank values the original keeps as NULL (tflip's are blank, as in the
    original's live one).
  - **Settings kept but not used**: `AdsStmtSetTableLockType` (tflip locks
    its own way) and `AdsStmtSetTableRights`.
    `AdsStmtReadAllColumns` is accepted and ignored. `AdsStmtSetTableReadOnly`
    makes live cursors read-only or not, as their open options show; an
    `AdsSet*` call on a read-only one fails with 5072, as on a static cursor
    (`fixtures/errors/cursor_set.json`; see Writes through a SQL cursor).
  - **Not probed**: keys of several columns of other types than text and
    dates (refused with 2011, as the original refuses a Clipper table's),
    and the lock type, rights and collation changed while a cursor is open
    (taken).

- **Not run yet** (the plan is in [TODO.md](TODO.md#sql)):
  - failing with 7200 (native 2200, "tflip does not support … yet"):
    the SQL debugger's commands besides
    `DEBUG BEGIN` and `DEBUG END`; `DEBUG RUN`, `BREAK`, `CONNECTION`
    and `REMOVE` fail so too where the original, not being a debugger, says
    2233;
  - the system procedures but `sp_GetTables`, `sp_GetColumns`,
    `sp_GetSQLKeywords`, `sp_GetSQLStatements`, `sp_CreateIndex`,
    `sp_CreateIndex90`, `sp_GetApplicationID`, `sp_SetApplicationID`, the
    catalog and collation procedures and the table, management and
    dictionary procedures below (the publication, article and subscription
    ones are not run); of the management family,
    `sp_AllowMultipleCollations` is not run yet;
- **The table system procedures** (`sp_PackTable`, `sp_ZapTable`,
  `sp_Reindex`, `sp_EncryptTable`, `sp_DecryptTable`,
  `sp_GetTableEncryptionType`, `sp_GetTableSizeInfo`, `sp_GetRecordCRC`,
  `sp_IgnoreTableTransactions` and the `…Online` forms, which run on
  `tflipd` only, the local server refusing them with 5042 as the original's
  does) run as the
  original's (`fixtures/sql/spt.json`, `sptdbf.json`, `sptdd.json`, and the
  files byte for byte), but: after
  a `SELECT` the original keeps the table open in its cursor cache, so
  packing, zapping, reindexing or encrypting it fails with 7008 there,
  where tflip's local server lets go with the cursor (see Open-table
  caches); in a transaction tflip refuses pack, zap and reindex with 5063,
  which the original's local server, having no transactions, does not
  (not recorded); `AdsVerifySQL` of a system procedure's call checks
  neither the number nor the types of its arguments; and table names are
  case-sensitive on Linux.
- **Query logging** (`sp_EnableQueryLogging`, `sp_DisableQueryLogging`,
  `sp_GetQueryLoggingResults`, `sp_ViewQueryLogging`) logs a connection's
  outermost statements as the original does (`fixtures/sql/spql*.json`,
  `spddql.json`, `spddql2.json`, the empty log table's files byte for byte
  in `spql6/`), but: the statements a trigger, a procedure or a script runs
  are rows of their own as the original's (`spddql3.json`, `spddql4.json`:
  the text as written, Optimized false but for a procedure whose body is one
  statement, the rows the script changed so far, the End Time of a
  procedure's last statement), but the End Time of a final `SELECT` is
  written when the script ends (not when its cursor closes), the End Times
  the original puts on a trigger's row after a DDL statement in a script
  are not reproduced, the statements a system procedure runs inside a
  script are not logged, and `sp_EnableQueryLogging('sub\ql')` in a
  dictionary makes the table `ql` at `sub\ql.adt` before it fails with
  7200 in the original (tflip fails without making it); on a free connection the
  log table is made plain and encrypted with RC4 when given a password, and
  a log already encrypted takes it (7060 when wrong; with none, statements
  fail with 5096 at their append and emptying it with 5104;
  `sp_GetQueryLoggingResults` 7060 and 5101), as in the original
  (`fixtures/aes/api/11_querylog` to `14_querylog_plain`), and a
  dictionary's log is never encrypted; not matched: enabling a log that
  exists unencrypted with a password (the original writes a password check
  into the header and `sp_GetQueryLoggingResults('')` then fails with
  5079), and `true`/`false` as the procedure's boolean arguments (tflip
  refuses them with 2124); an empty
  table name, or a path on a free connection, is refused; the minimum time
  is read as seconds, and where above it the original drops even the enable
  row (between 90000 and 95000) is approximated; an end time is written when
  the statement's cursor closes; and the log's writes belong to the user's
  transaction (not probed). `sp_mgGetTableCacheInfo` lists the files of the
  tables open here, where the original lists those its cache holds (the
  replay compares its columns only).
- **The catalog and collation procedures** (`sp_GetCatalogs`,
  `sp_GetSchemas`, `sp_GetTableTypes`, `sp_GetTypeInfo`, `sp_GetProcedures`,
  `sp_GetProcedureColumns`, `sp_SupportsConvert`,
  `sp_SupportsTxnIsolationLevel`, `sp_RetrieveSystemProcProperty`,
  `sp_GetCollations`, `sp_GetCollationTable`) run as the original's
  (`fixtures/sql/spcat.json`, `spddcat.json`, `spcoll.json`, `spcoll2.json`),
  and `system.systemprocedures` lists its 166 rows (shipped data, made from
  the fixture), but: `sp_GetCollations` and `sp_GetCollationTable` read the
  repository from the SDK's `adscollate.adt` (without it only the ICU rows
  are listed, shipped from the original's list, and `sp_GetCollationTable`
  gives 7147); `CollationID` counts the collations tflip's own session has
  loaded, where the original counts those of its process; a
  non-administrator of a dictionary is refused (5054) `sp_mgKillUser`,
  `sp_GetSQLStatements` and `sp_CancelQuery` and no other procedure, as in
  the original (what `Required_Permissions` names is not the rule); the
  replication procedures it names are not there to refuse; and `CREATE
  PROCEDURE … (x VARCHARFOX(n))` is refused with 2135 where the original
  takes it.
- **The dictionary system procedures** (users and groups, referential
  integrity, links, `sp_GetSecurityInfo`, `sp_DisableTriggers` and
  `sp_EnableTriggers`, `sp_ModifyDatabase` and the `sp_Modify…Property`
  ones, `sp_AddTableToDatabase`, `sp_AddIndexFileToDatabase`,
  `sp_RemoveIndexFile`, `sp_RenameDDObject`, `sp_MoveDDObjectFile`) write
  the dictionary byte for byte as the original does
  (`fixtures/sql/spdd*`), but:
  - `sp_GetSecurityInfo` gives the communication type `LOCAL` always;
  - not implemented: `sp_ModifyTriggerProperty` of `TRIG_CONTAINER_W` (the
    original stores bytes that are not the value), `sp_ModifyDatabase` of
    `DEFAULT_TABLE_PATH` and `TEMP_TABLE_PATH`; the validation
    options that check the rows of a table that has some (the fail table),
    and web delta and concurrency on a table with a `ROWVERSION` column;
  - asking for web delta makes the dictionary version 4 and turning it off
    rewrites every user's `Info2`; tflip does both from two observations
    (`marks_users` in `procedure/dictionary.rs`), without knowing the rule;
  - a `Max_Failed_Attempts` above 32767 makes the original's `SELECT * FROM
    system.dictionary` fail with 5179; tflip gives the value.
  - `system.dictionary`'s encryption columns are the original's for a
    dictionary it made (`fixtures/sql/ddenc.json`), and an RC4
    dictionary's table password, `ENCRYPT_NEW_TABLE` and `ENCRYPT_INDEXES`
    are set as the original sets them; the tables of such a dictionary are
    made, added (`AdsDDAddTable` encrypts the free table in place) and
    encrypted or decrypted as the original does (`fixtures/aes/rc4tp`, DBF
    tables too: y1 to y3, y6 to y9); an AES dictionary that encrypts
    new tables refuses CDX and NTX tables with 7168 as the original's, but
    gives a VFP table an AES layout tflip does not make (y4, not decoded);
    `AdsDDAddTable` of an ADT table bound to any dictionary is 5130, and
    of an encrypted one it cannot read 5138, 7060 or 7068, as in the
    original (z1, z2, y5); `AdsDDGetDatabaseProperty`'s
    ids 106 and 120 read the dictionary, 105 and 119 do not yet;
  - a connection that gave no user sees `system.dictionary` in full, where
    the original shows it only the public columns (seen on `connect e4.add`
    with no user, not replayed; other system tables not checked).
- **The management system procedures** (`sp_mgGet…`, `sp_mgKillUser`,
  `sp_mgResetCommStats`, `sp_SetStatementLimit`, `sp_SetRequestPriority`,
  `sp_CancelQuery`) run as the original's local server's
  (`fixtures/sql/spm.json`, `spm1.json`, `spm1dd.json`), but: the lists of
  connected users, open tables and indexes and locks show what the session
  has open, where the original lists its SQL cursors (`tim_N.tmp`) and the
  tables a statement holds too, so only their columns are compared;
  `sp_mgGetActivityInfo` gives `LoggedErrors` 0 (the original 1) and an
  uptime from the first call; `sp_mgGetServerType` gives the OS version
  0.0; `sp_SetStatementLimit` is kept and shown by `sp_mgGetConfigInfo`, and
  `tflipd` enforces it as the original's remote server does (7216 for a
  connection's 51st statement that ran and is not closed,
  `fixtures/sql/txlimit.json`), the local server not, as the original's
  local server does not; `tflipd`'s sessions show the remote server's
  `sp_mgGetConfigInfo` values (Burst Packets 16, Sort Buffer 8192, Receive
  IP Port 6262, ...), the ports the evaluation server's, not `tflipd`'s;
  `sp_CancelQuery` is 5042 in the client library, as on the original's
  local server; and `sp_SetRequestPriority` has no effect.
- **Scripts sent as a statement** (several statements, or one that starts with
  `DECLARE`, `IF`, `WHILE`, `RETURN`, `RAISE`, an assignment or `EXECUTE
  IMMEDIATE`) and **`EXECUTE IMMEDIATE`** run as the original runs them
  (`fixtures/sql/scripts.json`: the cursor of the last statement, the rows
  written together, positions in the whole text, the checks of the script
  language), and the same interpreter runs procedure and trigger bodies. But:
  - **Transactions** in a script are the original remote server's
    (`fixtures/sql/txscripts.json`); its local server has none.
  - **Script variables** take the types, sizes and assignment rules of the
    original's `DECLARE` (`fixtures/sql/declare.json`), but: a `ROWVERSION`
    variable shows as `BIGINT`; a `CICHAR` one compares regardless of case,
    where the original's compares exactly; a NULL `STRING` added to text is
    as long as the text (the original's 1); and a `VARCHAR` or `NVARCHAR`
    variable of more than about 1000 to 1500 characters shows its declared
    length, where the original's shows 1.
  - **The system variables** (`::conn.*`, `::stmt.*`,
    `::server.OldestActiveTxn`; `fixtures/sql/sysvars.json`,
    `trigvars.json`) are read as the original's, but:
    `::conn.OperationCount` counts the statements run, where the original
    counts operations of its own; the two numbers of `::conn.Name` follow
    the connection's number, where the original's first is 2 higher after
    each `AdsDDCreate`; and `ClientHostName` is empty on `tflipd`, where
    the original gives the client's machine name, which tflip's protocol does
    not carry (`NetworkAddress` is the client's address and `IsRoot` false,
    as there; `fixtures/sql/txremote.json`, not replayed).
  - **Procedures** read their input as `__input` and fail as 5154 as the
    original's do (`fixtures/sql/procvars.json`), but: `__input` cannot be
    written to (the original's can); a `ROWVERSION` or `MODTIME` output
    column and a NULL `ROWVERSION` input are not stamped; a local variable
    named as a parameter is not refused; the 5154 text is not cut at 564
    characters; a built-in system procedure called from a procedure's body
    fails; `CREATE TABLE` and `DROP TABLE` in a procedure fail with 5042,
    where the original runs them; a procedure that assigns an undeclared
    variable is created (the original refuses it at `CREATE` with 2218); a
    script sent as a statement cannot call a procedure (5004 on `__output`,
    the original runs it); and transactions in a procedure are the remote server's
    (`fixtures/sql/txprocs.json`), but not probed in a function or in an
    `EXECUTE IMMEDIATE` in a procedure's body. Parameter lists longer than 275 bytes continue in `More
    Property` as the original's do, the `.add` and `.am` byte for byte
    (`fixtures/sql/ddproclong`, `fixtures/dictionary/proclong`): the
    original sets a procedure's properties one at a time, and so do tflip's
    creations of procedures, views, triggers, functions and new tables'
    columns (`fixtures/sql/spddlong`).
  - **User-defined functions** run their statements on the calling
    statement's connection (`fixtures/sql/udf.json`), but: the rows an
    `INSERT … VALUES (f(…))` reports differ when the function writes (the
    original's 3 for one row written); a `RETURN` of a value of the wrong
    kind is not refused (2124 in the original); a function that reads a
    table with memo fields from a statement scanning that same table can
    deadlock; and a subquery with `ORDER BY` and no `TOP`, and a `(DELETE
    …)` or `(EXECUTE …)` in parentheses, fail with other parse errors.
  - **Nested triggers** that update the same record fail with 5035.
  - **`ALTER TRIGGER` to a script of 65,535 bytes** of a trigger that was
    made with one, then altered to a short one, reads back whole and fires
    in the original; tflip stores it unreadable and fails the statement
    with 5005 (`tools/ace/sql/trigbig.txt` records only a short one altered
    to that size).
  - **Procedures called by a trigger** (`fixtures/sql/trigproc.json`): a
    system procedure that returns a result set is skipped, as a trigger has
    no statement to build its cursor for; and a runtime error of a
    procedure's `INSERT` or `UPDATE` (a division by zero in an expression)
    is placed at its statement, where the original places it at the
    expression.
  - **A trigger made or dropped on a second connection** while another
    connection's statement holds the table is never seen by the original
    afterwards; tflip sees it when the statement closes
    (`fixtures/sql/txtrig3.json`).
- **`CAST(x AS SQL_CHAR)` without a length** is as wide as the original
  makes it (`fixtures/sql/castchar.json`, `castchar2.json`), but a
  `ROWVERSION` column is 21 wide in tflip where the original's is 20, and a
  `TINYINT` or `REAL` keeps its 4 and 13 only as the direct operand of
  another `CAST`, not through a derived table.
  - **A parameter without a value** read by an assignment or condition is NULL
    (the original fails with 2141 there; a statement that reads one still does).
    A parameter of the text of an `EXECUTE IMMEDIATE` takes the script's own
    value of that number, where the original has none to give it.
  - **`EXECUTE PROCEDURE` in a script** fails as the original's does (5004: the
    procedure's rows go to `__output`, which is no table), with two spaces
    where the original's text has four before `[SAP]`.
  - **A `SELECT` in the text of `EXECUTE IMMEDIATE`** is parsed and not run, as
    in the original.
  - **Cursors** (`DECLARE c CURSOR`, `OPEN`, `FETCH`, `CLOSE`, `WHILE` and
    `IF [NOT] FETCH`, `c.column`) run as the original's
    (`fixtures/sql/cursors.json`), in procedure and trigger bodies too, over a
    `SELECT` and over `EXECUTE PROCEDURE` (`cursorprocs.json`,
    `cursortrig.json`; a cursor over `EXECUTE PROCEDURE` in a script sent as a
    statement fails as `EXECUTE PROCEDURE` there does). A cursor over one
    table reads it live (`cursorlive.json`), and so does one sorted by one
    column the table has an index for (it takes the keys of one index leaf at
    its first `FETCH`, then goes on from the last record's key,
    `cursormore2.json`; with a `WHERE` that reads an indexed column, where the
    original may fix the records at `OPEN` by a rule not worked out, tflip
    fixes them at `OPEN`) and one over an inner join of two tables on one
    equality (`cursormore3.json`). A cursor over a `LEFT JOIN`, a join of three
    tables or more, a join with an `ORDER BY` or a condition on one table,
    `UNION ALL` and a `SELECT` of one table in `FROM` read live too
    (`cursormore4.json`), and so do a join with no equality (a cross join,
    `ON t.n < u.g`), a `RIGHT JOIN` of two tables on one equality, a `LEFT
    JOIN` of two tables with more than its equality in its `ON`, a `SELECT`
    of columns in `FROM` that is joined or has an `ORDER BY`, and a join of
    three tables with an index on a linking column or a condition on one of
    them (`cursormore5.json`). Still read into rows at `OPEN`: a join of
    three tables or more with no equality, a `LEFT JOIN` of three tables or
    more with more in its `ON` than its links, a `RIGHT JOIN` with an `ORDER
    BY` or a `WHERE`, a derived table with a condition on its own columns,
    and a joined `SELECT` in `FROM` that has an `ORDER BY` or is not of
    plain columns. Tables no equality links are looped with the last one
    outermost unless a condition on one leaves it fewer records (ties to the
    last; with a condition spanning both tables, the first is outermost); a
    `RIGHT JOIN` is the inner join, then the second table's records that no
    combination had (found against the keys it had at `OPEN`), with nulls. A
    join of three tables or more fixes at `OPEN` the first row's records but
    the last table's, and the records of its path still to find. With a
    `LEFT JOIN` or an `ORDER BY` the walk starts at the
    first table, else at the one a condition on it alone leaves the fewest
    records of (ties to the first): these rules, and that the first row's
    first record stays as it was at `OPEN` unless a condition other than an
    equality spans two tables of an equality join, fit
    the recorded cases but are inferred. In an index's walk, a record deleted in the loop whose slot an
    insert then takes is not shown as the original shows it. The rows a
    procedure called by a cursor's procedure writes are not added to the
    update count, `ROWNUM()` in a live cursor's select list is 1, and a
    cursor's unnamed expression columns are named otherwise.
  - **`TRY … CATCH [ALL | name] … FINALLY`**, `__errcode`, `__errtext`,
    `__errclass` and `RAISE;` run as the original's
    (`fixtures/sql/trycatch.json`), but: `SELECT __errcode AS c` in a
    `CATCH` gives the value, where the original gives a NULL `VARCHAR(1024)`;
    a `SELECT` that fails in a `TRY` fails at once, where the original fails
    it only when its rows are read (`TRY SELECT nosuch FROM t; SELECT 2 …`
    does not fail there); some syntax errors in a `CATCH` or `IF` body,
    `RAISE END` and a bad `FETCH` condition differ in their words or
    position; and `BEGIN TRANSACTION`/`ROLLBACK` around a `TRY` roll back in
    tflip (see Transactions above), which the original's local server does
    not.
- **Temporary tables** (`#name`) are the connection's own tables, in files of
  the system's temporary folder (`tim_<n>.tmp`, with the counter static
  cursors take) that go when it closes, of the table type of the statement that
  made them, by `CREATE TABLE`, `INSERT`, `UPDATE`, `DELETE`, `SELECT`, `CREATE
  INDEX`, `ALTER TABLE` (which drops the orders, and keeps the file's name,
  where the original's new file has another) and `DROP TABLE`, as the original
  answers them (`fixtures/sql/temptables.json`; they are never in a
  dictionary, on a dictionary connection too). One made as
  another type than the statement's is refused as the original refuses it,
  with 7016 (7028 between Clipper and FoxPro), unless the statement's last
  run was a `SELECT` that read it (`fixtures/sql/temptables_types.json`).
  `SELECT … INTO #t` makes the temporary table of the statement's table type,
  not a free ADT table as `INTO` does for a regular table: its fields in upper
  case in Clipper and FoxPro, as given in VFP and with every field taking NULL
  there; the DBF types refuse what they cannot hold (5012: Time, Raw,
  TimeStamp and VarBinary outside VFP, short integers in VFP) and names of
  more than ten characters (`fixtures/sql/temptables_into*.json`). A Clipper
  or FoxPro temporary table opens through `AdsOpenTable` as either until a
  statement has opened it, and after that only as that statement's type, 7028
  for the other (`fixtures/sql/temptables_api2.json`, `temptables_api3.json`;
  `INTO #t` is such a statement). Not probed: the definition in the 5012 of a
  too long name for a short integer, Time or Raw column; `INTO #t` in `SHOW PLAN`, in a script or in a procedure; whether a failed
  open releases a table's type (an `AdsOpenTable` as the other DBF type
  succeeded after one in a probe); a temporary table's size and where the
  original keeps its file (its cache keeps small ones in memory). Where tflip
  differs: a Clipper or FoxPro `ch + memo` is a Memo in the original, a
  VarChar in tflip, so an `INTO #t` of one makes a Character field there; and
  `CAST('text' AS SQL_TIME)` fails in tflip (2236) where the original gives a
  value.
- **The functions that read the row, the statement or the connection**
  (`APPLICATIONID`, `COLLATE`, `DELETED`, `EMPTY`, `LASTAUTOINC`, `LASTROWID`,
  `LASTROWVERSION`, `RECNO`, `ROWNUM`, `TTOC` and the `ROWID` column) work
  as the original's (`fixtures/sql/rowfuncs.json`). `CHR`, `PAD*`, `INLIST`,
  `TODAY`, `NOW_UTC` and `DESCEND` are for the expression engine only: SQL
  answers 2158 for them (as does tflip), and so for `COLLATE(s, 'ads_default_ci')`
  in its function form. A `ROWID` is the CRC-32 of the connection's path in
  upper case (the `.add` of a dictionary) and of the table's file's path (a
  dictionary table's object id) and the record number, each as six base 64
  digits (`fixtures/sql/rowid_paths.json`); the replay does not compare its
  first twelve characters, which follow from the path of its own folder. What
  differs:
  - `INSERT … VALUES (RECNO())` is 0 here, the number of the record it
    appends there; `ROWNUM()` numbers the rows after `DISTINCT` has sorted
    them there (here as they are read), and a `ROWNUM()` in an `ORDER BY` is
    the same for every row in both, but tflip's cursor has an index;
  - a live `SELECT` with `ORDER BY RECNO()`, or by a column given a
    collation, is a static cursor here, a live one on its table there;
  - `EMPTY` is NULL on an ADT table's column, the empty value of its type
    on a DBF table's (spaces only for text, but tabs and line ends too in a
    live `WHERE`, as the expression engine does; a memo is empty with no
    text), and false for anything else than a column; a column of a derived
    table is taken for an ADT table's (not probed);
  - `RECNO()`, `DELETED()` and `ROWID` of a table of a `SELECT` around a
    subquery are not found (not probed); `ROWID` is no column of a derived
    table or `system.iota` (2121, as there);
  - `COLLATE` takes `ads_default_ci` and `ads_default_cs` after a column or
    a string; texts of different collations fail with 2213 only where both
    were given one;
  - `LASTAUTOINC(STATEMENT)`, `LASTROWID()` and `LASTROWVERSION()` belong
    to the statement handle, which keeps what an `INSERT` (or, for the
    ROWVERSION, an `UPDATE`) last wrote to a table that has them, and
    `LASTAUTOINC(CONNECTION)` the connection's; `APPLICATIONID()` is the
    name of the client's executable, as the original's, and changes with
    `sp_SetApplicationID`.
- **The options of the DDL** run as the original runs them
  (`fixtures/sql/ddopts.json`, `ddoptsdict.json`, the files they leave
  byte for byte): `CREATE INDEX` with `CANDIDATE` (a unique order in an ADI
  file, a true unique one in a VFP index; refused for Clipper and FoxPro
  tables), `NOTDELETED` (the condition `!Deleted()` for a FoxPro or VFP
  table; none for an ADT table, and none for a Clipper one), `IN FILE`
  (dictionary tables only; the index file and its orders join the dictionary)
  and `PAGESIZE`; `CREATE TABLE … IGNORE TRANSACTIONS`; `ALTER TABLE … IGNORE
  | RESPECT TRANSACTIONS` (which restructures the table, and fails with 5152
  where it would change nothing), `DROP [CONSTRAINT] PRIMARY KEY`, `ALTER
  COLUMN … DROP …` (nothing to do for a free table, the dictionary's records
  for a dictionary's), `ALTER … TABLE`'s `NULL`, `NOT NULL` and `NOCPTRANS`;
  `DROP TABLE … FROM DATABASE [NO_DELETE]`; and `DROP INDEX` of a dictionary
  table's order. But:
  - **`ONLINE`** (`CREATE ONLINE INDEX`, `ALTER ONLINE TABLE`,
    `ALTER ONLINE_WITH_TRANSITION TABLE`) is refused as the original's local
    server refuses it (5042, 5152). Through `tflipd`
    (`fixtures/events/events_online.json`, `events_online_final.json`) the
    offline forms close the table to other sessions (7008; a dictionary's
    table is named without its file), and the `ONLINE` forms leave it open:
    `CREATE ONLINE INDEX` collects its keys and `ALTER ONLINE TABLE` builds
    the new table while others read and write, and a final stage (which
    signals `__OCIFinalStage_<table>` or `__OAFinalStage_<table>` and makes
    opens wait) builds again, in full, if the table changed meanwhile,
    where the original replays the changes; `ONLINE_WITH_TRANSITION` waits
    for the connections that opened the table meanwhile to disconnect.
    Inserts by others while an online operation runs append after the last
    slot instead of reusing the free list, so the records' order is the
    original's. A DBF table's `ALTER ONLINE` is gated but not staged; an
    exclusive open, and an open waiting in the final stage, give up after
    60 seconds (7008); and a new order is not known to another session's
    cached open of the table, whose writes after the final stage may not
    keep it current.
  - **`sp_PackTableOnline`, `sp_ReindexOnline` and
    `sp_PackAllTablesOnline`** run through `tflipd` as on the original's
    remote server (`fixtures/events/events_online_args.json`,
    `events_online_procs.json`). But they wait for a statement that holds
    the table: the original's final stage goes on while another connection
    has an unclosed `SELECT` cursor on the table (probed with `Timeout=1`,
    not in a fixture), where tflip's exclusive open counts the statement
    and the call ends with 7176 (pack) or 7189 (reindex) at the `Timeout`.
    `Indexes=` is checked for ADT tables only (5065 for the table's own
    `.adi`, 7041 for a missing file); an existing index file of another
    table is accepted and ignored, which the original may not do, and DBF
    index files are not probed. The original's stale 7175 (ADS_BUGS) is not
    reproduced: tflip queues a second online operation on a table until its
    `Timeout`.
  - **`ALTER TABLE` of a dictionary's table** that changes fields runs as
    the original's (`fixtures/sql/ddalter`, `ddalterdbf`, byte for byte):
    `ADD`, `ALTER` and `DROP COLUMN` with their constraints restructure it,
    on an ADT, Clipper, FoxPro or Visual FoxPro table, and make its Field
    objects anew; a change that only renames fields renames them in place
    without a `.BAK` (free tables too, `fixtures/write/restructure_rename`,
    `fixtures/sql/ddalterfree`; a Clipper or FoxPro name is upper case, so a
    change of case restructures there); the index files and Key objects
    (names, key lengths, key types) follow; a Clipper table's `.ntx` files
    are rebuilt; and the right to alter is checked (5054). `CREATE INDEX`
    and `sp_CreateIndex90` make the Key objects of a DBF table too. But: an
    order the new types cannot build (a field with an order becomes a memo)
    leaves the old table with no index file and a new memo file, as the
    original does (`fixtures/sql/ddalterfail`), its objects back in the
    original's record slots and the orders built before the failing one
    kept in the new index file (`fixtures/sql/spddalterfail`; with more
    orders than fields, `spddalterfail2`: the index files keep their slots
    and the Key objects, last to first, and the fields fill the others).
    With orders in several index files (`spddalterfail3`), the files
    before the failing one are built, the failing file keeps the orders
    before the failing one (gone if the failing order is its first), and
    the files after it stay as they were.
    `AdsRestructureTable` of a dictionary's table takes the same path
    (`fixtures/write/restructure_fts_dict`). A restructure that follows a
    failed one rebuilds the orders from the dictionary's Key objects when
    it changes, drops or moves a field an order uses, and leaves the index
    file alone otherwise, as the original does
    (`fixtures/write/restructure_fts_dict_after*`). What differs: the
    dictionary's objects after a failed drop of an order's field (other
    slots, one id fewer; `restructure_fts_dict_drop`, its test ignored);
    the memo blocks of the full-text Key objects of the dictionary's last
    table, which the original leaves where they are after a failure (the
    fixtures end with a table that does not fail); an order on an NChar or
    NVarChar field cannot be made from its Key object
    (`AE_NOT_IMPLEMENTED`); the rebuild from Key objects is for ADT tables
    only (a DBF table's not probed); SQL on a table the same connection
    holds open by `AdsOpenTable` fails with plain 7008, where the
    original's is 7200 with native error 7008 and `Table name: x`; and
    `ALTER COLUMN … VarChar` of a Char field with a full-text order fails
    with 5066 where the original converts.
  - **The dictionary's `.ai` after some single-statement inserts**
    (`fixtures/sql/ddapirights`, `ddapirights2`, `ddrenamerights`,
    `spddalterfail3`, `ddrepl`) has the original's keys in other pages: the
    original splits a nearly full leaf at another key than tflip, by a rule
    not found yet (see TODO).
  - **A free connection** refuses the dictionary's tables in the DDL
    statements (5159, as the original does), but opens them by `AdsOpenTable`
    and `SELECT`; and a dictionary connection opens a free table of the
    dictionary's folder by its name, where the original says it is no table
    of the dictionary (5132).
  - **Not probed**: the original's odd readings of `PAGESIZE 5.5`, `PAGESIZE
    0x200` and `IN FILE sub\name`, the order of its refusals among the
    options of `ALTER TABLE` beyond those in the fixtures, `IGNORE
    TRANSACTIONS` on the tables of a dictionary of another type than ADT, and
    which of two refused full-text options (5019) the original names first.
- **`SET ::conn.Collation` and `SET ::stmt.Collation`** (`fixtures/sql/setcollation.json`)
  check the name as the original does and have no effect, as in the original,
  where a client's statements open their tables with what `AdsSetCollation` and
  `AdsStmtSetTableCollation` say; the other variables fail as unrecognized
  (2225) or, for a scope other than `conn` and `stmt`, as an invalid
  assignment (2124). `SET ::conn.Collation = NULL` crashes the original
  engine; tflip fails it with 2124.
- **`TOP n PERCENT`** keeps the share of the rows rounded up (`fixtures/sql/percent.json`),
  and `RAND([seed])` draws from the connection's own generator, the
  original's own sequence number for number (`fixtures/sql/funcs.json`),
  with `TOP` too, which works out the select list only for the rows it keeps,
  after every row's sort keys. But in `WHERE n < RAND() * 10` the original
  draws fewer numbers than it reads rows, by a rule the probes did not make
  clear (tflip draws one a row); and
  `RAND(0)` seeds from the clock's milliseconds, which the original counts
  from another start (a tick count), so the two seeds differ.
- **Full-text search** runs in SQL: `CONTAINS()`, `SCORE()`/`SCOREDISTINCT()`
  and the ordinal `SCORE(N)`, in the select list, `WHERE` and `ORDER BY`, over a
  field's full-text index, `CONTAINS(*, …)` over every full-text field, and a
  non-indexed fallback (`fixtures/sql/fts.json`); a FIXED (not maintained) index
  is read as the original reads it, from its keys where its AOF answers a
  `CONTAINS` and from the record's text where not (`fixtures/sql/fts_plans.json`).
  `SHOW PLAN` shows the original's plans: a live cursor's AOF with its
  `CONTAINS` terms among the other predicates, optimized and counted by the
  full-text orders (a word as long as the index's longest word, a phrase or
  `NEAR` only narrows), and a static cursor's (`SCORE`, `{static}`) `EVALUATE`
  over an AOF of its restrictions. What differs: a static cursor shows no
  restriction on an expression (`id + 1 = 2`) or `IS NULL` beside a `CONTAINS`,
  nor an `IN` beside one that no index serves the same way, and the static
  plans tflip makes otherwise without a `CONTAINS` (an index's scope with another
  restriction, an `ORDER BY` an index serves, `TOP`, joins, `COUNT(*)`) stay as
  below.
- **Joins, grouping, `UNION` and subqueries** run on rows read into memory
  (`tflip_sql::query`), as the original engine answers them in
  `fixtures/sql/select.json`, but:
  - **The original engine's optimizer** picks which table of a join to loop
    over first. With a `WHERE` on the right table only, or with joined
    `SELECT`s in `FROM`, its rows can come in another order; tflip swaps
    only an inner join of two tables by size (see Indexes in `SELECT`s below)
    and otherwise loops over the left table first.
  - **A `SELECT` of one table in `FROM`** (`SELECT * FROM (SELECT id FROM
    t WHERE …) x WHERE …`) is a live cursor on that table, as in the
    original (`fixtures/sql/derived.json`): through nested ones, with the
    columns renamed or reordered, and with writes; `{static}`, `TOP`,
    `DISTINCT`, expressions, grouping, `UNION` and joins make it static.
    An `ORDER BY` in a derived table or subquery without `TOP` is refused
    (2115) as the original does, but not in a subquery of the select list
    (worded otherwise there). A statement with parameters keeps its derived
    tables static.
  - **Which value stands for a group** of CICHARACTER values that differ only
    in case: the original engine shows one tflip cannot always tell.
  - **Rows with equal sort keys** come in another order in some `ORDER BY`s
    on expressions that are not result columns.
  - **Integer averages** are the original's: rounded half to even, and the
    sum instead when the average is negative (`AVG(-id)` over 1 and 2 is
    -3). MONEY and NUMERIC averages are cut off.
  - **Joins, `GROUP BY` and `UNION` read every row of their tables** into
    memory, with no index, and only equality conditions between two
    tables avoid comparing every pair of rows.
- **`INSERT`, `UPDATE` and `DELETE`** write as the original engine does
  (`fixtures/sql/dml.json`, the files byte for byte but for the memo file),
  `UPDATE` and `DELETE` reading only the records the table's orders find
  for their `WHERE` (as a live cursor's), but:
  - **The rows a failed statement affected** (`AdsGetRecordCount` on the
    statement): the original engine reports them erratically, the count
    before, 0 or 4294967295 by kinds of statement and error tflip cannot
    tell apart; tflip counts the rows written before the error, and
    4294967295 if none were.
  - **A record a unique index refuses** has its memo values written to the
    memo file first in the original engine, their blocks staying used;
    tflip writes nothing, so the memo values after it lie elsewhere in the
    `.adm`.
  - **No transaction** surrounds a statement: one that fails keeps the rows
    it wrote before the error, as in the original engine without one.
  - **`UPDATE … FROM`** (`fixtures/sql/updfrom.json`, the files byte for
    byte) updates each record of its table once, from the first row of the
    join it is in, and sees the writes before it; `WITH DELETE` and `WITH
    RECALL` work as the original's (an ADT table's deleted records are never
    read again; an ADT record written `WITH DELETE` frees its memo blocks).
    But `FROM (EXECUTE PROCEDURE …)` is not supported; the plan (`SHOW
    PLAN`, `fixtures/sql/planupd.json`) has the original's steps, but the
    original joins the target first, writes the conditions of the `JOIN`s and
    the `WHERE` as one step with the later table first, and shows `Join
    Condition<…>` on the scan, and an `UPDATE` or `DELETE` whose `WHERE` an
    index serves is an AOF SCAN there, an INDEX SCAN here; and a view or linked table as its table and triggers on
    it were not probed.
- **`MERGE`** runs as the original engine does on ADT, Clipper, FoxPro and
  Visual FoxPro tables and on a dictionary connection
  (`fixtures/sql/merge.json`, `mergedd.json`; the files byte for byte): each
  source row, in the order the source reads, updates the target rows its
  condition holds for or inserts one, and sees what the rows before it wrote
  (a source that is the target too); text cut to fit fails the statement at
  its end; `INSERT … DEFAULT VALUES` and `DEFAULT` in a list of values work
  (in `INSERT` too); `WITH DELETE` keeps the values in an ADT record's
  deleted slot. But:
  - **A condition is looked up by scanning the target** for every source
    row, never by an index.
  - **The plan** (`SHOW PLAN`) is the join of the source and the target with
    the original's loop (never a hash join) and a step per clause, but a
    column shows as its table's field name and the alias as declared, where
    the original shows the statement's spelling (a DBF table's `[t].[ID]` for
    `t.id`), and a `SELECT` in `USING` with a `WHERE` is two scans, not one.
  - **A source that is a `SELECT` of other tables** is read once, not as the
    statement goes; one of the target table is read again after each row
    written.
  - Memo values of a record written `WITH DELETE` are not probed; and the
    original's local server leaves a deleted blank record for each INSTEAD OF
    trigger's insert, which tflip does not (`fixtures/sql/mergedd/inst.adt`
    is not compared).
  - A table with several triggers has its two aggregate slots in the
    dictionary (`ADS_DD_TABLE_TRIGGER_TYPES`, `…_OPTIONS`) differently from
    the original (45 and 51 for BEFORE and AFTER, INSERT and UPDATE
    triggers, where tflip's OR makes 5 and 3), as `mergedd.json` found.
- **`CREATE TABLE`, `CREATE INDEX`, `DROP TABLE`, `DROP INDEX`** and
  `sp_CreateIndex90` make and remove free ADT tables and their orders as
  the original engine does, the files byte for byte (`fixtures/write/ddl`,
  `fixtures/write/build` for orders built on tables with records,
  `fixtures/write/bitmaps` and `fixtures/write/cdx_bitmaps` for a binary
  order's bitmap of several pages),
  and DBF tables (see SQL on DBF tables above); `SHOW PLAN FOR` them fails
  with 7200 (native 2200).
- **`CREATE TABLE` and `DROP TABLE` on a dictionary connection** add the
  table to the dictionary and take it out as the original engine does,
  the dictionary and the files byte for byte (`fixtures/sql/ddtables`):
  its fields with their `CONSTRAINT NOT NULL`, `DEFAULT`, `CONSTRAINT
  MINIMUM`/`MAXIMUM`/`ERROR MESSAGE`, `MEMOBLOCKSIZE`, the `PRIMARY KEY`
  (a column's or the table's, named by `CONSTRAINT name`), `IN DATABASE`
  and `AS FREE TABLE`, the objects in deleted objects' slots, and the
  refusals. Not probed: `CREATE TABLE`
  on a dictionary with a default table path (tflip makes the files beside
  the `.add`), an encrypted dictionary's tables, and `DROP TABLE` of a
  table with referential-integrity rules (tflip leaves them; its triggers
  and permissions go with it). The SQL trigger tests still run on the psm fixture's tables:
  `fixtures/sql/triggers.json` was recorded on the remote server, which
  making their own tables would need again.
- **`SELECT … INTO table`** makes a free ADT table of the result's columns and
  fills it, whatever the statement's table type, on a free and on a dictionary
  connection (beside the dictionary, not in it), and returns a live cursor on it;
  a statement that fails leaves no table (`fixtures/sql/selinto.json`, the files
  byte for byte in `fixtures/sql/selinto/`). A connection whose
  EncryptionType is AES, or one of a dictionary of AES tables, makes it
  laid out for AES and not armed, as `CREATE TABLE` does
  (`fixtures/aes/dictwrite/48_select_into`). `SHOW PLAN FOR` such a statement and
  `INTO` inside a script or a procedure were not probed. `INTO #name` makes a
  temporary table instead (see Temporary tables).
- **`EXECUTE PROCEDURE` in `FROM`** (`(EXECUTE PROCEDURE p(args)) alias`) reads
  the result of a script procedure or of a system procedure as a static cursor's
  table, in joins, conditions, `GROUP BY` and subqueries, once per statement
  and when it executes (not when it is prepared or verified), the last one in the
  `FROM` first; a procedure with no output parameters, a `CURSOR VARYING` one,
  and a system procedure with no result set are refused (2235)
  (`fixtures/sql/execproc.json`). But arity is checked only for script
  procedures, `sp_GetTables`, `sp_GetColumns` and `sp_GetApplicationID` (the
  ones probed); what is left of external procedures is in
  [TODO](TODO.md#3-the-larger-pieces-l-each); `SHOW PLAN` of such a statement was not probed; and the
  original's `EXECUTE PROCEDURE numbers(3)` as a statement reports its 3 rows
  as affected (the `AdsGetRecordCount` of its statement), where tflip's 0.
- **An external procedure's tables through the table API**
  (`fixtures/sql/aeptables.json`, `aeptablesremote.json`): as on both of
  the original's servers, `__output` does not open (7112) and the plain
  names `__input`, `__output` and `__error` are 7041, whose path text is
  not probed.
  `AdsGetRecordCount`, `AdsGetRecordNum` and `AdsAtEOF` on `__error` do not
  post a record left appended, as the original's do. `AdsGetLong` and
  `AdsGetString` of a field of another type fail with 5066 without the
  original's `Field: "x"` detail, and `AdsOpenTable` of a `#name` that is
  not there on a dictionary connection is 7112 where the original's is 5132.
- **What an external procedure's library may do with its connection**
  (`fixtures/sql/aeprules.json` of the original's local server,
  `aeprulesremote.json` of its remote server, which `tflipd` answers as;
  see [TRANSACTIONS.md](TRANSACTIONS.md)): on the local server its
  transaction calls succeed; on the remote server it may commit or roll
  back only the levels it began (5047), and a refused call spoils the
  caller's transaction. But: only procedures are probed, so a trigger
  library gets no such rules in `tflipd`; outside the caller's
  transaction, a library's SQL `COMMIT` or `ROLLBACK` after its own `BEGIN`
  is answered 5047 though it takes effect, and in it the `ROLLBACK` is
  refused with 7200 and the level stays (recorded, not explained); a
  library's `AdsGetConnectionPath` in `tflipd` gives `tflipd`'s local path,
  not the server's; two client processes that chose the same range of
  handles are not told apart. A library's own `AdsConnect60` in `tflipd`
  is a connection of type 4 as the remote server's (types 1, 2 and 3 are
  refused with 6082 and 6097) and reaches the local session of the
  library's copy of the client library, which reads the files in
  `tflipd`'s process outside the server's locking, with transactions as
  that copy's `tflip.ini` says.
- **Small differences the grammar probes found** (`fixtures/sql/gram_*.json`,
  where the binary literals, shifts, `{OJ …}`, the script forms `ELSEIF`,
  `ENDIF`, `ENDWHILE`, `END TRY` and a bare `END`, `CHAR_LENGTH … USING`,
  `::stmt.unicode_sort_key_size` and `::conn.TerminalClientAddress` run as
  the original's): tflip answers as the original does for a constant
  number, text or bytes where a logical is wanted, the bitwise operators on
  bytes and on `ABS`, `CAST`'s type names and sizes, bytes cast to numbers,
  `COALESCE` of bytes and the syntax errors of a missing operand
  (`fixtures/sql/gram_cast*.json`, `gram_syntax.json`), but: `1 << (0 +
  ABS(-2))` is pointed at the `+` there; the original answers 2119 for a
  subquery as an `AND` operand and for eight bytes cast to a BIGINT
  (ADS_BUGS), where tflip gives the value; and a ROWVERSION cast to bytes is
  its text, zero-padded; `a CROSS JOIN b CROSS ON a.id = …` (`CROSS` as
  both tables' alias) is 2132 for the alias used twice, where the original
  resolves the `ON` first and answers 2122 (`fixtures/sql/gram_cross.json`
  has the rest of `CROSS JOIN` as the original's). Not probed: `CHAR_LENGTH(…
  USING CODE_POINTS)` against `CHARACTERS` for a character outside the BMP
  (the probe script cannot hold one), and a VARBINARY column given too long
  a binary literal (tflip cuts it, 2102).
- **The SQL edge cases** (`fixtures/sql/claims.json`, `claims_huge.json`):
  924 of 944 answer as the original's; the other 20 are listed in
  `claims_known.txt` and `claims_huge_known.txt`, a `# <case>` above each.
  In groups: the 5070 of `CAST('1e2' AS NUMERIC)` and of `CAST` of a braced
  GUID, a double cast to text (16 digits where the original gives 14); a
  `UNION` of CICHARACTER with text (which of case-equal rows is shown); the
  5005 of 167 `ORDER BY` terms; `FROM "t2.adt"` aliases, a variable that is
  also a column (2229), the row order of `SELECT * FROM t2 a, t2 b`;
  the bitwise operators' remaining cases; the position of a script's
  runtime error; and `SHOW PLAN` of `DISTINCT`, `UNION` and `MERGE`.
  `LEAVE` and `CONTINUE` end or continue the innermost `WHILE`
  (`fixtures/sql/loops.json`); the original has no labels. `SELECT … WHERE
  n = ?` with a string or double parameter of more decimals than the
  NUMERIC column fails with 2102 where the original says 7200 "The result
  value exceeded the given buffer size.", and `UPDATE … SET v = ?` with a
  cut parameter and no matching row succeeds where the original fails with
  2102. A `THEN` of a temporal `CASE` that mismatches
  is worded "in not compatible" where the original says "is not
  compatible.". `NEXTROWVERSION` answers 5125 even on a dictionary connection,
  and `COALESCE` of an integer and a NUMERIC takes the wider precision
  where `CASE`, `IFNULL` and `ISNULL` keep the older rule. Of the
  grammar, a group in parentheses after `UNION ALL` is flattened, so `(A
  UNION B) UNION ALL C` differs where `ALL` and distinct `UNION`s mix; only
  the `vendor(…),product(…)` form of the `--(*vendor…*)--` escape is read;
  and `SHOW ALL INTERNAL PLAN` shows the plain plan, not the original's
  internal steps. The original cuts the text of a 2114 or 5012 for a name over 1024 characters
  (ADS_BUGS), tflip does not. `SHOW PLAN`'s statement numbers count the
  statements of script loops in the original, not in tflip (masked in the
  replay). An expression nested thousands deep (12,000 parentheses, which
  the original takes) overflows tflip's stack, and in a debug build so does
  a 128-term chain of `+`.
- **A `WHERE` run as a filter**: as the original's expression engine does,
  `AND`, `OR`, `IIF` and `COALESCE` in the `WHERE` of a live cursor, a
  table's scan, `UPDATE` and `DELETE` evaluate every operand, and a
  division by zero fails with 4120 (`fixtures/sql/wherefilter*.json`). But
  the original evaluates ahead of a skip, so its error comes earlier
  (`AdsSkip` from the first of 40 records fails when record 20 divides by
  zero, a skip back from record 25 and `AdsGotoBottom` too, by a rule not
  worked out); and of a table an index lookup restricts, tflip filters
  only the records the lookup finds.
- **Indexes in `SELECT`s that run through their plan**: as in the original
  engine, a table's restrictions (`=`, `<>`, `<`, `<=`, `>`, `>=`,
  `BETWEEN` and a logical column, against constants, parameters or
  columns of the `SELECT` around) are looked up in an ADI order on the
  column, and so is the right table of an inner or left join on an equal
  column; a table whose only restriction is one `>=` or `<=` (the whole
  `WHERE`) has its rows ordered by that column, with or without an order,
  equal values in record order (CICHARACTER text in lower case; the key
  order of an order on several fields that starts with the column); one
  `=`, `>=` or `<=` beside conditions that are no restriction (a `LIKE`) is
  read in the order's order; the others in record order
  (`fixtures/sql/select.json`, `indexsel*.json`). Orders on several fields
  are used by their first field, with `=` on their leading fields as one
  scope; ranges of CICHARACTER text and of text under a collation are
  served by the order (a collated range is scanned, not looked up, and its
  plan counts show NA); an `IN` list is scanned as the original scans it;
  and of an inner join of two tables the one with fewer records (a
  restriction counts as making it smaller) is read first
  (`fixtures/sql/joinorder.json`, `joinsize.json`). But tflip uses no order
  whose key is an expression, a condition or custom (the plan shows `TABLE
  SCAN` where the original's shows `AOF SCAN`), does not look up an
  expression on the left of a join's equality, never swaps three tables or
  an outer join, and does not pass a restriction on one table of a join on
  to the other's AOF as the original does; and one pair is unexplained: a
  12-record indexed table first against a 10-record plain one is not
  swapped by the original, and of two tables of the same size joined with
  a comma and no order (`FROM x1, x2`) the original reads the second in the
  outer loop, tflip the first. Its plans differ in the AOF text's order of
  conjuncts, the order of the detail lines and the `EVALUATE` of a join's
  condition (`indexsel.json` is compared by operator, table and order
  only).
- **Live cursors** use indexes as the original engine's do: the `WHERE` is
  the AOF the original makes of it (`AOF<id=2 .AND. name="Bravo">`,
  `tflip_sql::xbase`), whose comparisons the orders find (looked up as
  ranges of keys, so that moves pass the other records by), and an `ORDER
  BY` whose columns are an order's key reads the records in that order
  (`fixtures/sql/plans.json`). But a `WHERE` tflip cannot write as an AOF
  (a parameter, functions other than `UPPER`, `LOWER` and `CURDATE`) is
  evaluated on every record, and its plan shows it as not optimized. (Of
  an `.AND.` of three lookups and more, both leave out those after two
  that leave one record, `Actual: NA`; the rule is taken from one probed
  plan.) Which `WHERE` makes a cursor static is the original's for every
  scalar function of the help's lists (`master_indexes_with_expressions`;
  `fixtures/sql/statics.json`): the functions the help says disqualify an
  index, `TRIM`, `CASE`, `ROWID` and `RAND` make it static, the others do
  not; only the live or static choice is probed, not the AOF each
  condition becomes. A `LIKE` that is the only condition gives its rows
  ordered by its column, as the original's (`fixtures/sql/likeorder.json`,
  `likedict.json`; the dictionary's `system.` tables keep table order),
  and so beside conditions an AOF cannot hold; the rule beside other
  conditions is taken from the probes (`like_order.rs`), and a parameter
  pattern beginning with a wildcard is not probed.
- **Query plans** (`SHOW PLAN FOR`) are the plans tflip runs, in the
  original engine's plan cursor (its columns, row types, operators and
  wording), so ARC draws them; they differ from the original's where it
  plans otherwise (`fixtures/sql/plans.json`): tflip
  joins on equal columns through a hash where no index finds the right
  rows (`Hash Join<…>` on the join, no warning that no index serves it,
  each table scanned once), swaps the tables of an inner join of two tables as the original does
  (the smaller first) but not of three, keeps a join's
  condition in the join rather than in a `WHERE`, passes no restriction on
  to a static `SELECT` in `FROM`, reads the tables of a join with a derived
  table by a hash or a scan where the original swaps them or reads the
  derived table's base table by an index, writes a derived table's `WHERE`
  and the outer one as `a .AND. b` (the original `(b).AND.(a)`, as for
  views), lists a repeated `COUNT(*)` once, makes no AOF of what a `LIKE`
  begins with, and shows steps the original leaves out: the plan of the
  select list's subqueries (one that reads the rows around it says it runs
  for each). A `COUNT(*)` alone, and a derived table's `SELECT`, show no
  scan, as in the original (`fixtures/sql/derived.json`). The original's
  plan numbers count DDL and `INSERT`; tflip's do not. The original's plan cursor has an index
  (`AdsGetNumIndexes` is 1); tflip's has none.
- **The SQL debugger**: `DEBUG BEGIN` and `DEBUG END` keep a connection's
  debugger state, and the other commands fail as in the original engine on
  a connection that is not debugging (ARC sends `DEBUG END` after each
  statement); on one that is, they and the commands with arguments
  (`DEBUG RUN conn`, ...) fail with 7200 (native 2200).
- **System tables**: `SELECT` from the `system.*` tables works on a
  dictionary connection (`system.objects`, `system.fts` and
  `system.systemgroups` on any), but what
  is encrypted is not read — see
  [Data dictionaries](#data-dictionaries). `sp_GetColumns` with a pattern
  such as `inde%` lists the original's system tables (`system.indexes`) too.
  `sp_GetTables`/`sp_GetColumns` on a dictionary connection list the
  dictionary's tables and views, its links' tables and the system tables as the
  original does, views described (`fixtures/sql/spview.json`). Not probed: a
  catalog argument naming the dictionary by its path (tflip matches it as a
  `LIKE` pattern), and the catalog and type arguments on a free connection
  (tflip ignores them there). A view's nullability rules are learnt from the
  expressions probed; functions that may return NULL from values that are not
  (none was found) would be reported not nullable.
- **Long properties of removed objects**: removing a view, procedure,
  table, field, user or group frees the `More Property` memo block and then
  the comment's, as the original does (`fixtures/dictionary/removelong`);
  so do removing an order, a trigger or a field with long properties and
  `ALTER TABLE … ADD`, `ALTER` and `DROP COLUMN` with them
  (`fixtures/sql/spddlongalter`, `spddlongidx`), and so do an order's own
  comment (`spddidxcomment`, `spddidxcomment2`) and Field continuations of
  more than 32 blocks after `CREATE TABLE` and `ALTER TABLE`
  (`spddlongalter2`, `spddlongalter3`).
- **Temporary indexes** are added as in the original engine
  (`fixtures/sql/orderby.json`): `TIM_<n>` on a live cursor whose `ORDER BY`
  no order serves, and on a static cursor sorted on its columns (not with
  fewer than two rows, a `TOP`, or the keys of a `GROUP BY` alone). They
  answer `AdsGetNumIndexes`, `AdsGetAllIndexes`, `AdsGetIndexHandleByOrder`
  and `AdsGetIndexHandle` (by name, and for no name as the cursor's current
  order, which a live cursor that reads one of its table's orders has too),
  with their name, key expression, flags and file; the cursor's moves
  through their handle are its own. A static cursor whose `ORDER BY` an order
  of its table serves (`… LIKE … AND id > 3 ORDER BY id` with an index on
  `id`) is read in that order with no `TIM` index, as in the original
  (`fixtures/sql/staticorder.json`), by a rule taken from the cases probed:
  one `ORDER BY` column, a table of the `FROM` with an order on it, and no
  `LIKE` reading in another column's order; compound orders and a join's
  right table were not probed. `AdsSeek` and `AdsSeekLast` through one
  look the key up among the cursor's records' keys, as the original does
  (soft and `SEEKGT` seeks, strings, doubles and dates, a key that starts
  the keys it finds), but a scope set on one (`AdsSetScope`) does not limit
  the cursor's moves, as the original's does; and their key length and type
  are worked out from the fields, as probed for the keys tried. The order a
  live cursor reads backwards (`ORDER BY id DESC` on an index) is turned
  round in its own handle, as the original's is (`AdsIsIndexDescending`,
  `AdsGetIndexHandleByExpr`, `AdsGotoTop`, ...); but `AdsSeek` with `gt`
  through such a handle (or after `AdsSetIndexDirection`) lands after the
  key sought, where the original lands on the first key at or below the
  next one's. An `ORDER BY` of
  what the original's expression engine does not write (`CAST`, `IIF`,
  `TRIM`, `ROUND`, `MOD`, `CASE`, `CONCAT`, text of two columns added, and
  functions not probed) is sorted in a static cursor; the original's index
  sorts the text of `IFNULL`/`COALESCE` of a CICHARACTER column regardless
  of case, tflip's sort does not, and `MOD` sorts equal keys otherwise.
- **Static cursor names**: `tim_<n>.tmp`, numbered on from 1 in the
  process; the original's numbers differ.
- **Values too large for their column**: a static cursor fails with 5179
  for the first row that has one and leaves later rows out without an
  error, keeping their record numbers and then counting no records, as the
  original does (`fixtures/sql/gram_syntax.json`), also with `DISTINCT`,
  `GROUP BY`, joins and `UNION`; one with an `ORDER BY` fails. But the
  original raises 2111 (`CAST` to an integer type) and 2236 (`CAST` to a
  NUMERIC) for the first row where tflip gives 5179 or clamps, drops any
  later row that errors in its projection and not only an overflow at
  storage, renumbers the rows of a derived table, and rejects `123` for a
  table's own `NUMERIC(3,0)` column with the `Field:` text where tflip's
  setter gives a bare 5179 (an `INSERT … SELECT` names the field `EXPR_1`).
- **Syntax errors** are worded and placed as the original does for the
  cases of `fixtures/sql/gram_syntax.json`, but `!!` is one token there
  (tflip names `!`).
- **Writes through a SQL cursor**: a live cursor (one table's columns, also
  through a view) takes `AdsAppendRecord`, `AdsDeleteRecord`,
  `AdsRecallRecord`, `AdsWriteRecord`, the setters and the record locks into
  its table, as the original's does, its `ORDER BY` following the records
  written (`fixtures/write/cursor_*`); a static cursor, a join's and a live
  one of a read-only statement refuse with 5072 (the setters after
  converting the value for the column, `fixtures/errors/cursor_set.json`),
  and `AdsLockTable`, `AdsPackTable`, `AdsZapTable` and `AdsReindex` on any
  cursor are 5115. A column that is not there is 5113 (by name) or 5112 (by
  number). `AdsStmtConstrainUpdates` refuses a write that leaves the `WHERE`
  (7065); `AdsStmtReadAllColumns` is accepted and does nothing, as on the
  original's local server. `AdsGetField` of a binary field of a cursor the
  original does not write answers an empty text there, where tflip gives the
  bytes. A live cursor of fewer or reordered columns keeps the table's ADI
  orders current (`SELECT name, id FROM t`). Tables opened by name take writes (see [Writing](#writing)). What
  differs:
  - The original answers some things from the AOF its `WHERE` becomes, where
    tflip evaluates the `WHERE` record by record: after an append to a cursor
    whose `WHERE` an index answers, `AdsGetRecordCount` does not count the new
    record there (`fixtures/write/cursor_where`, listed in its `known.json`);
    with an equality an index answers, the count does not change on a pending
    delete; and `AdsCancelUpdate` of a record that left a `WHERE` evaluated
    record by record is 5085 there (tflip cancels it).
  - A cursor on a Clipper or FoxPro table writes only as far as tflip writes
    those tables; only VFP and ADT were probed.
  - A cursor with an `ORDER BY` does not see a record another handle
    appended until its own next write.
  - `AdsCloseSQLStatement` of a statement already closed is 5106 here, 0
    there.
- **`sp_GetSQLStatements`** numbers statements on from 1 in the process
  (the original's numbers differ); on a server it lists every client's
  statements on the same dictionary or folder, its `Connection Name` the
  OS user, not the client's machine.
- **The application ID** starts as the client program's file name, as in
  the original; on `tflipd` it is the server's until a client sets it.
- **The place of a syntax error** counts a CR LF as one character, as the
  original does, but a CR LF inside a string literal is placed otherwise
  by the original (column 41 against 42, line 3 against 4).
- **Literals and conversions** behave as the original's
  (`fixtures/sql/literals.json`), but: `DATE'…'`, `TIME'…'`,
  `TIMESTAMP'…'` and the `{d}`, `{t}` and `{ts}` escapes are read when the
  statement is parsed, in the default `MM/DD/CCYY` format (whether the
  original takes the connection's is not probed); text compared with a
  date, time, timestamp or GUID fails when a row is evaluated, where the
  original's static cursor and derived table fail when the statement runs
  (a live cursor's `WHERE` fails when it opens, in both);
  `CAST('<GUID text>' AS SQL_GUID)` gives a GUID, where the original fails
  with 5070 (and matches nothing in a `WHERE`); `GUID'…'`, no literal in
  either, is refused at another place (12, the original's 26); and `SELECT
  +col` of a numeric column is a static cursor with an `EXPR` column, where
  the original's is a live cursor with the column's name. `INLIST` is no
  SQL function (2158) in either.
- **Parameters** are typed as the original types them
  (`fixtures/sql/params.json`): `- ?` and `NOT ?` fail with 2154, `? IS
  NULL` with 2123 even where the name is typed elsewhere (so Entity
  Framework's `(:p IS NULL OR col = :p)` fails in both; `CAST(? AS …) IS
  NULL` passes), `COALESCE` takes no parameter (2159, `<Parameter not
  allowed: Argument N>`) and `IFNULL` none untyped (2123), a date slot of
  `TIMESTAMPADD` and `TIMESTAMPDIFF` needs a `CAST`, and a character
  parameter is `VARCHAR(1024)` (over 1024, a `+` fails with 2155). A
  re-prepared statement keeps its values by position until
  `AdsClearSQLParams`, and takes parameter numbers up to the most any of
  its texts had (a name that comes twice is two parameters, which the name
  sets both of; `AdsGetNumParams` counts every marker, also of SQL that
  does not parse; `fixtures/sql/params4.json`, `params5.json`); `CASE` and the results of `IIF` take no parameter (2159),
  a sign, `~`, `NOT`, `AND` or `OR` of one is 2154 even when its name is
  typed, and `? IN (?, …)`, `? BETWEEN ? AND ?` and `? LIKE ?` are 2123
  (`fixtures/sql/params2.json`). Text in a numeric place, numbers set for a date
  and `AdsSetDate`'s text are converted as the original does
  (`fixtures/sql/params3.json`). But `CAST(date) + ?` of a long is 2124
  here (the date plus days there); `AdsSetDate`'s text is read where the
  parameter is used, so text that no date format takes is 2109 only where
  it is cast; only `UPPER` and `LOWER` cut an over-wide parameter (the other
  string functions are not probed); and `SELECT id FROM cust WHERE
  (CAST(? AS SQL_INTEGER) IS NULL OR id = ?)` is a static cursor in the
  original, a live one here.

## Writing

ADT, Clipper, FoxPro and Visual FoxPro records are appended, changed,
deleted and locked as the original engine does it, through every setter,
with their memo values and their ADI, NTX, CDX and IDX orders kept current
(`fixtures/write/`, replayed byte for byte), locally and through `tflipd`.
But:

- **What is not written yet** fails with `AE_NOT_IMPLEMENTED` before
  anything changes:
  - tables with an open order tflip cannot keep current: orders on an
    expression over an NChar or NVarChar field that tflip cannot build
    (see NChar and NVarChar orders), dynamic collations without
    their files, a full-text order on an NChar, NVarChar or NMemo field
    (whose keys are ICU sort keys) — one made `NOTMAINTAINED` is no
    hindrance;
  - a binary order whose bitmap has more leaves than the root over its
    interior nodes holds (62 × 62 leaves, over 15 million records, with nodes
    of 512 bytes): the original's third level is not probed (the second is,
    `fixtures/write/bitmap_levels`), so one that is built that large, or
    grows past that, fails.
- **Full-text orders are built and kept current** as the original does,
  byte for byte (`fixtures/write/fts_build`, `fts_writes`, `fts_rebuild`):
  `CREATE INDEX … CONTENT` with each option, `AdsCreateFTSIndex`, their
  refusals, appends, changes and deletes (each word added or taken away
  counted in the tag header), `AdsReindexFTS`, `AdsReindex`, pack and zap.
  In a transaction on an ADT table (`fixtures/write/fts_tx_rollback`,
  `fts_tx_commit`, `fts_tx_big_*`, `fts_tx_memo_*`, `fts_tx_exclusive`) the
  orders hold no word of it until the commit: `CONTAINS` on the connection
  that writes and on another reads the order as it was, a rollback leaves
  the `.adi` untouched, and the commit takes each write's words in as the
  original does. FoxPro and VFP tables are the same
  (`fixtures/write/fts_tx_cdx_*`, `fts_tx_vfp_*`, the files only).
  `CONTAINS` on a FoxPro or VFP table finds what the order holds
  (`fixtures/aes/rc4tp` y11, y12); inside a transaction on a DBF table it
  is not tested. Binary, Image and VarBinary fields (`fixtures/write/fts_binary`, a zero
  byte splits words) and FoxPro and VFP tables (`fts_cdx_create`,
  `fts_cdx`, `fts_cdx_null`: a tag of the structural `.cdx` for SQL, a
  single-order file of its own for `AdsCreateFTSIndex`) are made and kept
  current too, and deletes and recalls, one by one and all at once, are as
  the original's (`fts_recall`, `fts_recall_binary`). But a rebuild counts
  a scored order's words right where the original counts some wrong (see
  [ADS_BUGS.md](ADS_BUGS.md)); of a Binary table where a record was deleted
  and never recalled, a later `AdsRecallAllRecords` leaves one count byte
  of the `.adt`, `.adm` and `.adi` other than the original's (not
  explained, likely in the freed memo blocks); and replacing an existing
  CDX or IDX order with `AdsCreateFTSIndex`, `AdsCreateFTSIndex` on a
  Clipper table and restructuring a table with a CDX full-text tag (which
  loses its options block) are not probed. A Clipper table has no
  full-text orders (`CREATE INDEX … CONTENT` is 5017, as the original's).
- **Dictionary-bound AES tables** are made as the original makes them in an
  AES dictionary (`fixtures/aes/dictwrite`): laid out for AES, and armed
  under the dictionary's key after `ENCRYPT_NEW_TABLE`. A *free* AES table is
  made too (a connection with `EncryptionType=AES128`/`AES256`, then
  `AdsEnableEncryption` and `AdsEncryptTable` — verified end-to-end both
  ways against the patched engine). An existing table of an AES dictionary is
  encrypted and decrypted with its memo values and index files
  (`TABLE_ENCRYPTION`, `sp_EncryptTable`, `sp_DecryptTable`;
  `fixtures/aes/dictwrite` 08 to 15). The memo file of any table laid out
  for AES keeps its slabs' records, free runs and values behind IVs (see
  DIFFERENCES for the bytes of a freed run). RC4 encryption
  writing is complete (see the README status).
- **A handle without an encrypted table's password** reads and writes as the
  original's does (`fixtures/write/encryption/share.json`): the password is
  the handle's, not the open file's. On an AES table it is probed
  (`fixtures/aes/api`, `tools/ace/crypto/aesapi.sh`) and matches: the
  record it reads is blank (text, Money, Numeric and Logical fields as
  spaces, a variable length as -1, the rest zero), its orders find keys (a
  record's key is the blank one, so `AdsGetKeyNum` is 5041), a change,
  delete or recall is pending and fails 5096 on every move until
  cancelled, index creation and reindexing fail 5096, encrypting and
  decrypting 5096 (before the open mode is checked), packing 7068, and a
  zap empties the table before it fails 5096. Left: the RC4 forms of pack,
  reindex, index creation, encrypt and decrypt (not probed; tflip keeps its
  older answers, 5096 for a pack); what such a handle sees of a record
  another handle deleted (the original lists it in a move and counts it,
  tflip hides it); and memo values written between `AdsEnableEncryption`
  and `AdsEncryptTable` are enciphered at once in tflip, where the original
  arms the memo file only at `AdsEncryptTable` (they read the same; their
  bytes are not compared).
- **A dictionary's table opened as a free table**: as the original, tflip
  refuses any ADT table whose header names a dictionary (5159, "The table is
  a database table"), whether the dictionary is there or not
  (`fixtures/write/encryption/freeopen.json`); an RC4-encrypted structure is
  7060, or 5159 while its dictionary has it open, as in the original.
  An AES dictionary's table opened free is 7068 whatever the connection's
  keys (in SQL inside 7200), and 5159 while its dictionary has it open, as
  in the original (`freeopen.json` `aes_t_*`, `aesc_t_*`). In
  the original, a dictionary's encrypted CDX table opens as a free table
  (records enciphered, `AdsIsEncryptionEnabled` false); not checked in tflip.
- **A dictionary's encrypted table over `tflipd`** opens through the
  dictionary: the server releases its password from the login it holds (an
  AES dictionary's from the `DDPassword` sent with the connect). Writing to
  the RC4 fixture's table fails locally too (5042, an order tflip cannot
  keep current; not investigated).
- **Deleting orders** (`AdsDeleteIndex`, `DROP INDEX`): of an ADI file
  and a `.cdx` as the original does it (`fixtures/write/cdx_drop`), the
  last order of a `.cdx` taking the file with it; the order of an `.ntx`
  or `.idx` file is not deleted (`AE_NOT_IMPLEMENTED`, not probed).
- **Creating and maintaining tables** is checked against the original for
  ADT tables, through the ACE API and SQL (`fixtures/write/maintenance`,
  `fixtures/write/ddl`, `fixtures/write/table_ops`), and for DBF tables
  through the ACE API: `AdsCreateTable` (`fixtures/write/dbf_create`),
  `AdsCreateIndex` for NTX and CDX orders (`fixtures/write/ntx_create`,
  `fixtures/write/cdx_create`, and `fixtures/write/cdx_kinds` for `.idx`
  files and custom and binary orders), `AdsPackTable`, `AdsZapTable` and
  `AdsReindex` (`fixtures/write/dbf_maintenance`,
  `fixtures/write/dbf_rebuild`). Not checked: a reindex fills a custom CDX
  order as any other. `AdsCreateIndex` with a null tag fails in tflip
  (`AE_INVALID_POINTER`, 5021) where the original builds an `.ntx` order
  (its file has no tags); a null tag for an ADI or CDX file is not probed.
- **An append a VFP candidate order refuses**: the original engine adds
  the record's keys to the orders before the candidate one, and leaves the
  record deleted at the close; tflip refuses the record before writing any
  key (`fixtures/write/cdx_indexes` has candidate orders whose refusals
  agree).
- **The record buffer after a write**: `AdsGetRecord` on a record appended
  in the session gives a status byte of 0 in the original engine, and the
  next four bytes as `AdsSetRecord` last set them, even after the record is
  written; tflip gives the record as the file holds it (status 4, the
  prefix zeros). Clients do not look at those five bytes.
- **Switches taken but not acted on**: `AdsDisableRI` and `AdsEnableRI` change
  nothing, as the original's local server does: a dictionary's rules always
  apply. (`AdsDisableUniqueEnforcement` and
  `AdsDisableAutoIncEnforcement` are now acted on — see the changelog — except
  that a CDX-candidate duplicate inside an explicit transaction stays enforced.)
- **Recalling records** (`AdsRecallAllRecords`) takes their memo values
  back from the memo file's free space as the original does for values of
  up to 32 blocks in a slab still in use; a larger value, or one in a slab
  freed whole, stays free (not checked against the original).
- **Code pages**: `AdsSetStringFromCodePage` converts from the single-byte
  Windows and OEM pages the original accepts and the four double-byte pages
  (932/936/949/950), with the engine's own tables; a page it has no converter
  for fails as the original does (1565). See Connections and configuration.
- **The original's limits** (`fixtures/write/limits`, `fixtures/sql/limits.json`,
  `limits_procs.json`): field lengths and decimals, record length, memo
  block sizes (5058), names, key and expression lengths, open index files,
  tags, index columns, procedure scripts and the nesting of procedures and
  triggers are refused as the original refuses them. But a table's path
  over 255 characters (2009) is judged on tflip's own path, not the
  Windows form the original sees; and the 64 nested procedures and
  triggers the original allows run on the caller's stack, which a 2 MB
  thread of a debug build cannot hold (release builds not measured).
- **Restructuring** (`AdsRestructureTable*`, `ALTER TABLE`) changes free
  ADT tables as the original does (`fixtures/write/restructure`: fields
  added, deleted, renamed, moved and converted between types, the
  structural index rebuilt, the old files kept as `.BAK`) and free DBF
  tables (`fixtures/write/dbf_restructure`: every record kept, the
  structural `.cdx` rebuilt, NTX files left alone, as the original does).
  A table with full-text orders is rebuilt too, the orders with their
  options: each order is made and built before the next, as `CREATE INDEX`
  lays the pages (so a large order's pages come out as the original's,
  full-text or not); a field that cannot hold a full-text order is refused
  with 5066 before anything changes on a free ADT table, after the records
  are converted on FoxPro and VFP tables, and when the order is built on a
  dictionary's table (`fixtures/write/restructure_fts*`). But: the restructure of a dictionary's table is under SQL (**`ALTER TABLE`
  of a dictionary's table**); other index files of a free table stay as
  they are; `ALTER TABLE`'s column constraints are
  dropped; its `MEMOBLOCKSIZE` rewrites the memo file as the original does
  (ADT 8 to 32768, FoxPro 1 to 1024, else 5058; an unchanged size or no
  memo field 5152; dropped unread after two column actions), on a
  dictionary's table too, whose `Table_Memo_Block_Size` it sets, compared
  byte for byte by hand but with no fixture; where the original fails
  `IGNORE` or `RESPECT TRANSACTIONS` beside two column actions and a
  `MEMOBLOCKSIZE` (2240, or 2004), tflip runs it; its `IGNORE`
  and `RESPECT TRANSACTIONS` restructure a free table, and a dictionary's ADT
  table that way as well (see **The options of the DDL** under SQL). Conversions between types not probed (GUID, Raw,
  VarChar, NChar, ...) follow the rules of those probed.
- **Copying tables** (`AdsCopyTable`, `AdsCopyTableStructure`,
  `AdsCopyTableContents`) copies to a table of the same type as the
  original does (`fixtures/write/copies`); an ADT copy's memo file has
  8-byte blocks whatever the source's, as in the case probed.
  `AdsConvertTable` converts every field type each table type has, from
  and to ADT, Clipper, FoxPro and VFP tables, as the original does
  (`fixtures/write/converts`, `converts_ext`, `converts_dbf`,
  `converts_values`): the field types and widths, names, NULL and empty
  values, deleted records, the order memo values are written in, the
  refusals (a General or Picture field to another kind of table, 5012) and
  a conversion that stops part way (a number too wide, 5179; a Clipper
  General value, 7019). Not probed: OEM tables, encrypted sources, a
  conversion onto a file another handle has open, and filters other than
  `ADS_IGNOREFILTERS` and `ADS_RESPECTFILTERS` with deleted records
  hidden. An ADT table's legacy ADS VarChar (`ADS_VARCHAR`) is now read
  as text and written by `AdsConvertTable` (its overflow allocated as
  probed for one value), but the setters still do not write one.
- **Deleting a table** (`AdsDeleteTable`) deletes its file, its memo file
  and its structural index file; other index files open on it stay (not
  checked against the original).
- **Bytes the original leaves to chance** are not the original's: the
  byte after a binary value in a `.dbt`, and the bytes of an `.ntx` page
  that hold no item, which the original fills from its own memory. tflip
  writes 0x1A after binary values, and zeros or the page's old bytes on
  `.ntx` pages; readers never look at either.
- **OEM tables in other code pages**: what clients write to an OEM table is
  converted to their OEM code page as the original does it for code page
  437 (its best-fit table, recorded in `fixtures/write/dbf_oem`); under
  other code pages tflip uses Windows' own best fit, unchecked.
- **Locks are tflip's own**: record and table locks, exclusive opens and
  writes lock byte ranges of tflip's own on the files
  (`tflip_storage::lock`), so tflip's clients in any process keep each other
  out, but the original engine does not see them, nor tflip its: do not
  share files between the two while either writes.
- **Other processes' appends** are seen when a handle goes to the top or
  bottom, counts records or writes (when the header is read again); a
  handle moving record by record meanwhile stops at the last slot it knew.
- **Index files only another handle has open** are not kept current by a
  handle's writes: each handle keeps the orders it has open (the structural
  index's, which every handle opens, among them).
- **Wide record numbers**: a leaf's record numbers are widened as the
  original does it the first time (past 16,383 records for 1-byte keys,
  65,535 for 8-byte ones); further widenings follow the same rule, not
  checked against the original.
- **Other handles see changes at once**: a record another handle wrote, or
  appended, is read as it is now. The original's handles keep what they
  read last until they lock, refresh or write themselves: one sees a record
  count that leaves out another handle's appends, and goes to a record
  another handle deleted.
- **Index pages other processes change** are read again by a handle's
  writes, which lock the table, but an order a handle walks keeps the pages
  it read: an NTX order's root in particular, which a split moves, is read
  again only when the order is opened again. The original compares each
  file's change counter.
- **Opening writes in the original**: the original engine writes to a
  table it opens for changing (not read-only), so the file's date changes
  and `AdsGetLastTableUpdate` reports the day it was opened; tflip leaves
  the file alone and reports its last write.

## Online backup

`sp_BackupDatabase`, `sp_BackupFreeTables`, `sp_RestoreDatabase` and
`sp_RestoreFreeTables` work on `tflipd` as the original's remote server
does, full and differential, with archives, and answer 5042 on the local
server as the original does (`fixtures/backup`, see its README). But:

- **AES** dictionaries and tables are not backed up or restored as the
  original does: the evaluation server cannot load OpenSSL (5214, 7160),
  so nothing was recorded (`s90_aes`, `s91_aes_free`).
- **Opening a dictionary** whose `d.ai` is missing makes it in the
  original, on any connection (`s04_open_image`); tflip makes it only at
  a restore.
- **A statement's table type** stays the connection's default for later
  statements in the original (`s03_dd_objects`, `s08_restore_extra`);
  tflip does not keep it. `sp_BackupFreeTables` opens tables as the
  connection's `TableType`, never a statement's `AdsStmtSetTableType`.
- **A free table missing** from a dictionary connection's folder is 5004
  in tflip, 7041 with the server's path in the original.
- **The server's error log** (`ads_err.adt`) gets no entries; the
  original logs 2, 7041 and 5168 for each table a restore creates.
- **The 7131 row of the dictionary file** (a backup while another runs)
  names the file by its path on the server, not by the original's
  `\\host:port\share\...` path.
- **A user's password change** also rewrites the dictionary's first row
  (the database's) in the original, with new random bytes, marking it
  changed for the next `Diff`; tflip leaves that row alone, so a `Diff`
  does not copy it.
- **Not probed, chosen:**
  - an archive that fails for another reason than a long name (a folder
    that cannot be made) gives the three rows of an empty name;
  - a connection opening a table a backup holds gets 7131 (from the help);
  - a backup whose dictionary another backup holds copies no dictionary
    file, `MetaOnly` too;
  - a stamp of 0, on the table or its image, never matches (6709);
  - a free-table `include=` naming a table twice gives 7131 for a DBF
    table and nothing for an ADT table (one case of `s40_free_backup`);
  - a free table converted to another DBF flavour has plain field types
    only; others give 7016;
  - restoring an encrypted table from a differential image.
- **Fitted, not understood**: in a `Diff`, the memo value of the record
  in slot 1 always takes a chunk of its own, its old one freed after the
  pass, where every other record's old value is freed at once
  (`s92_diff_memo_slot`, `s93_diff_memo_passes`, `s97_diff_memo_first`).
  tflip does the same, but why slot 1 is the exception is not known.
- **Not probed**: a `Diff` of free DBF tables (copied whole, as a
  dictionary's are), and an archive with a `Diff`.

## Remote server

- **Authentication and authorization**: a dictionary connection over `tflipd`
  checks the login password (the client forwards it over TLS and the server
  verifies it, as for a local connection), and a dictionary that verifies access
  rights enforces them per table for that user. A **free-table** share, though,
  takes any user and enforces nothing, so anyone who can reach the port and open
  such a share reads it.
- **Connection paths on Windows** are made full and, through a drive mapped
  to a share, the share's UNC path, as in the original (`tools/ace/mapped`):
  `X:\data` with a remote server type goes to `tflipd` on the share's host.
  A drive's root stays `C:\` where the original reports `C:`.
- **Timeouts**: connecting has a timeout (5 s by default), and a per-request
  read/write timeout can be set so a stalled server does not block a client
  forever (`[connection] request_timeout` / `TFLIP_REQUEST_TIMEOUT`, in
  seconds; off by default, as the original engine's client blocks
  indefinitely). Both are configurable (see `crate::config`).
- **Compression against a server that says never**: a client that says
  `ALWAYS` gets an uncompressed connection, as the help's table has it; whether
  the original server refuses it with 7084 instead was not probed
  (`tools/ace/compress`).
- **Free tables**: a server refuses tables on connection 0 with
  `AE_FILE_NOT_ON_SERVER`; clients reach tables through connections only.
- **Open-table caches** (`AdsCacheOpenTables`, `AdsCacheOpenCursors`): a
  connection to `tflipd` keeps closed tables open on the server as the
  original server does (`tools/ace/cache`), with these differences:
  - the **local server keeps none by default**: the setting is accepted and
    ignored there, though the original's local server caches exactly as its
    remote one does; `tflip.ini`'s `[cache] local = yes` makes it cache the
    same way, closing the statements' tables at once when their size drops
    to 0 as the original's local server does (on purpose, see
    [DIFFERENCES.md](DIFFERENCES.md));
  - a kept table holds its file open on the server (or, with local caching,
    in the client's process), so **another process** cannot open it
    exclusively until it is evicted, as with the original; the same process's
    own connections evict it;
  - tflip keeps **files, not cursors**: `AdsCacheOpenCursors` N counts the
    files of statements' tables, not statements, and the result set is not
    reused (a repeated `SELECT` reads again, from a file already open);
  - a statement holds the tables its `SELECT` read until it closes or runs
    again, as the original does, but with the local server's caching off it
    lets go of them with its cursor (so other programs can open them
    exclusively); only a `SELECT`'s tables are held (what the original holds
    for other statements, such as `EXECUTE PROCEDURE`, was not probed);
  - `DROP TABLE` refuses a table open in the same process (7200, native
    7008) as the original does, but deletes one only another program has
    open (on Linux, where nothing stops it);
  - lowering the cursor size to 0 leaves the statements' tables kept, as on the
    original server (its local server closes them); tflip then closes them
    when the connection's next statement table closes;
  - exclusive and encrypted tables are never kept (invisible but for speed).
