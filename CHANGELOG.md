# Changelog

What each release added. The details of what works, and how it is
checked against the original engine, are in the README's
[status](README.md#status) and [`docs/KNOWN_ISSUES.md`](docs/KNOWN_ISSUES.md).

## Unreleased

## v0.26

- Online backup and restore on `tflipd`, as the original's server does
  them (`fixtures/backup`): `sp_BackupDatabase` and `sp_RestoreDatabase`
  of a dictionary's ADT, FoxPro, Visual FoxPro and Clipper tables (RC4
  encrypted too), `sp_BackupFreeTables` and `sp_RestoreFreeTables` (masks,
  `TableTypeMap`, the connection's `TableType`, passwords), differential
  backups (`PrepareDiff`, `Diff`; ordinary writes, transactions, pack,
  zap, `ALTER` and encryption mark the records and headers as the
  original does), tar and tar.gz archives byte for byte, the result
  rows and errors, tables held exclusively or by another backup (7008,
  7131) and a pending transaction left out. On the local server all
  four answer 5042, as there.
- A free AES table's memo values written before `AdsEnableEncryption`
  read as they are until `AdsEncryptTable` enciphers them (they read as
  garbage), and `AdsEncryptTable` no longer enciphers a second time the
  values written once the table was armed.
- A table of an AES dictionary opened as a free table is 7068 (5159 while
  its dictionary has it open), whatever the connection's keys
  (`fixtures/write/encryption/freeopen.json`).
- A handle without the password of an AES table reads the blank record the
  original gives it, finds keys in its orders, keeps a change, delete or
  recall pending until it is cancelled (5096 on every move), is refused
  index creation, reindexing, encrypting and decrypting with 5096 and
  packing with 7068, and zaps the table before failing with 5096
  (`fixtures/aes/api`).
- `AdsCreateTable` returns the table open exclusively, `AdsEncryptTable`
  of a free AES table with records enciphers them and their memo values,
  and the first index file of a free AES table is in the clear.
- `SELECT … INTO` on an AES connection, or of an AES dictionary, makes a
  table laid out for AES (`fixtures/aes/dictwrite/48_select_into`).
- Query logging with an encryption password on a free connection encrypts
  the log table and takes and checks the password as the original does
  (`fixtures/aes/api/11_querylog` to `14_querylog_plain`).
- A connection to `tflipd` sends its `EncryptionType` (a new protocol
  operation, `set_encryption_type`).
- An AES dictionary's index is updated in place, key by key (its pages are
  deciphered, changed and enciphered again; only the changed pages take a
  new IV), and a dictionary's index is built order by order, so a
  converted dictionary of more than about a hundred objects lays out its
  pages as the original does (`fixtures/aes/convert`, letter V).
- A handle without the password reads an armed AES VFP table as blank
  records and is refused appends and writes, as the original does;
  `AdsCopyTable`, `AdsCopyTableContents` and `SELECT … INTO` of AES tables
  write tables laid out for AES in the clear, as the original's;
  `SELECT … INTO` under a VFP table type makes a VFP table; and
  `AdsRestructureTable` of a table with a password is 5096
  (`fixtures/write/dbf_aes_nopassword`, `fixtures/aes/rc4tp` v13 to v16).
- Who may change a user's password follows the original: the old password
  is asked for unless `REQUIRE_OLD_PASSWORD` is off (5054 through
  `sp_ModifyUserProperty` and `AdsDDSetUserProperty`'s
  `ADS_DD_USER_PASSWORD`, 5097 for a wrong old one); another user's
  password is for ADSSYS, `DB:Admin` members and holders of `ALTER`, and
  only ADSSYS changes ADSSYS's; `AdsDDSetUserProperty` sets
  `ADS_DD_USER_PASSWORD` and `ADS_DD_CURRENT_USER_PASSWORD`;
  `sp_ModifyUserProperty` is allowed to holders of `ALTER`; and `USER()`
  works in a procedure's arguments (`fixtures/sql/spddpwchange`).
- A `DELETE` or `UPDATE` needs the read right only where its condition
  reads the table (5054), a subquery over the table it changes is 7079,
  and a column the user may not read in a subquery's select list in a
  condition is 5171 (`fixtures/sql/ddinsertonly3`).
- The permissions and effective permissions filtered by `Object_Type = 4`
  list the first table's columns only, and a `DB:Admin` member's
  `sp_GetColumns` lists a good view next to a bad slot, as in the original
  (`fixtures/sql/ddeffperm`, `ddpublicbad9`).
- The local server counts no failed logins, as the original's, and its
  7078 for a known user, ADSSYS or no user names `axServerConnect`
  (`fixtures/events/lockout_local.json`).
- FoxPro and Visual FoxPro tables of a dictionary: `CREATE INDEX …
  CONTENT` records its Index and Key objects, and `CONTAINS` finds the
  words of a CDX full-text order, in a clear and an RC4 dictionary; in an
  encrypted table, Binary and Image values keep their SIx tag in the clear
  and encipher their data; after `AdsDisableEncryption` an exclusive
  handle's current record reads as it was loaded, and a change to it is
  refused (5104); and a memo block past the end of
  the file fails with 7010 (`fixtures/aes/rc4tp` y11 to y14,
  `fixtures/write/dbf_encryption_cached_*`).
- Visual FoxPro tables of an AES dictionary, and free ones made on an AES
  connection, are laid out for AES and encrypted as the original does: an
  IV in each record's `_NullFlags` and behind each memo value, armed by
  `ENCRYPT_NEW_TABLE`, `AdsDDAddTable`, `sp_EncryptTable` and
  `AdsEnableEncryption`; they are read, written, packed, restructured and
  decrypted (`fixtures/aes/rc4tp` v1 to v12). CDX and NTX tables stay
  plain, as in the original.
- A restructure of a dictionary's table after a failed one builds the
  orders from the dictionary's Key objects when it changes, drops or moves
  a field they use, as the original does, and a failed restructure takes
  the Key objects' slots by the order of their records and frees their
  full-text memo blocks a file at a time
  (`fixtures/write/restructure_fts_dict_after*`).
- `AdsSetFieldRaw` on a Numeric field of an ADT table fails with 5070 for
  bytes that are no number (5179 for one too large), a VarChar value keeps
  its length bytes, and a DBF number or date is padded with blanks
  (`fixtures/errors/param_n.json`).
- Publications, articles and subscriptions answer as the original does:
  deleting a publication that subscriptions use is 5132; a missing
  publication, table or subscription, a duplicate name and a queue that is
  no file (5127, which uses up an object id) are told in its words; a
  queue name without an extension gets `.adt`, and one with a folder makes
  no files; deleting a subscription whose queue file another took over is
  5159; comments are written over the old block or stored once it is
  freed; and `system.publications`, `system.articles`,
  `system.subscriptions` and `AdsDDFindFirstObject` of types 15, 16 and 17
  list them (`fixtures/sql/ddrepl`, `ddrepl2`).
- `AdsDDDeployDatabase` deploys a source with subscriptions (their queue
  tables with them and their files in the folder), and one with a
  database comment next to rights granted to `DB:Public`, stopping with
  5136 as the original does (`fixtures/dictionary/deploy/x_sub*`,
  `x_dbc*`).
- A handle left on a record that a rollback to a savepoint freed keeps the
  record's image (and, in an ADT table, its record count); a whole
  rollback leaves an ADT handle on a freed slot at BOF with the record
  number past the last slot; and a DBF table's commit after such a
  rollback no longer answers 7138 (ADT only, as in the original)
  (`fixtures/write/tx_cursor_after`).
- A memo value of a Clipper, FoxPro or VFP table written while another
  handle holds its record's image goes to new blocks, once, as the
  original's server puts it (`fixtures/write/memo_other_handle`).
- RC4 encryption of Clipper, FoxPro and Visual FoxPro tables: free tables
  (`AdsEnableEncryption`, `AdsEncryptTable`, `AdsDecryptTable`,
  `AdsEncryptRecord`, `AdsDecryptRecord`) and the tables of an RC4
  dictionary (`ENCRYPT_NEW_TABLE`, `AdsDDAddTable`, `sp_EncryptTable`,
  `sp_DecryptTable`, restructure) are enciphered, read and written as the
  original does, byte for byte; an encrypted table's memo fields without
  the password are 5096, `AdsEncryptRecord` on a VFP table is 5096, and
  the detail texts of 5096 (append, encrypt, decrypt) and 5104 are the
  original's. `AdsDDAddTable` keeps a table inside the dictionary's folder
  by its relative path. Records written in a transaction are enciphered
  (the blank a rolled-back append leaves stays clear), and memo values
  are enciphered and read at their record's number in transactions, SQL
  conditions and `UPDATE` and `DELETE` (`fixtures/write/dbf_encryption_*`,
  `fixtures/aes/rc4tp` y1 to y3, y6 to y10).
- The dictionary's index (`.ai`) is kept up to date key by key after every
  write, as the original does, where it was built anew: its pages split
  and empty as they did through the dictionary's history, so after
  renames, DDL and `ALTER` it is the original's byte for byte
  (`fixtures/sql/spddrenamerepl2`, `spddrenamedbf`, `spddlongalter3`,
  `ddbf`, `ddcreatedbf`, `ddoptsdict`, `ddalter`, `ddalterdbf`,
  `ddalterfail`, `spddalterfail`, `spddalterfail2`). A dictionary that
  encrypts its index still has it built anew.
- A member of `DB:Admin` is the administrator for the listings that read a
  `DB:Public` slot that does not check, and sees every object; in the
  effective permissions it has the group's level with an Inherit of 2;
  `sp_GetColumns` lists no columns of a table or view whose slot does not
  check; and `sp_GetProcedures` fails with 5132 for an ordinary user
  (`fixtures/sql/ddpublicbad6`, `ddpublicbad7`, `ddpublicbad8`).
- A restructure of a table with full-text orders (`AdsRestructureTable`,
  `ALTER TABLE`) rebuilds the orders with their options, on free ADT,
  FoxPro and VFP tables and on a dictionary's tables, encrypted or not,
  instead of failing with 3015; a restructure builds each order of an
  index file before making the next, so the pages of large orders are the
  original's, full-text or not (`fixtures/write/restructure_fts*`,
  `fixtures/aes/dictwrite/47_restructure_fts`).
- `AdsDDAddTable` of an ADT table that is bound to a dictionary is 5130
  whatever its structure (tflip added it to the new dictionary and
  encrypted it), and 5138, 7060 or 7068 for an encrypted one it cannot
  read; a taken name is 5132 and uses up an object id. `AdsCreateTable`
  of a DBF table on a dictionary connection adds it to the dictionary. AES
  dictionaries that encrypt new tables refuse CDX and NTX tables with
  7168. `AdsConnect101` reads keys up to the next `=` and knows
  `Description` (`fixtures/aes/rc4tp` z1 z2 y5).
- `tflipd` locks out users whose logins fail too often, as the original's
  remote server does: with the dictionary's `ENFORCE_MAX_FAILED_LOGINS` on,
  the failure that reaches `MAX_FAILED_ATTEMPTS` disables the user's
  logins (7113 after, with any password); the count is kept in memory only,
  a good login or an administrator enabling the user again do not reset
  it, and ADSSYS is locked out too, until `tflipd` restarts. A user whose
  logins are disabled is told 7113 before the password is looked at, and
  7078 on a server ends in "axServerConnect" for every user
  (`fixtures/events/lockout_*.json`).
- `tflipd --check-free-table-access` and `--disable-free-connections` are
  the server settings `Check_Free_Table_Access` (7087 for every dictionary
  connection opening a free table, ADSSYS's too) and
  `Disable_Free_Connections` (7083 for free connections, ADSSYS's too)
  (`fixtures/events/freeconf_*.json`).
- `AdsDDDeployDatabase` of a dictionary with a table's rights granted to
  `DB:Public` fails with 5136 and leaves the dictionary the original
  leaves: the tables freed again unless an integrity rule names them, the
  groups kept, the users not added (`fixtures/dictionary/deploy/x_pubt*`).
- `AdsDDRemoveTrigger` takes the qualified name `table::trigger` only, as
  the original does (a bare name is 5132); `AdsDDAddIndexFile` of a
  missing file is 7041; a property id the original does not know is 5126
  (5042 for a user group) before the name and rights checks; and
  `AdsDDDeleteSubscription` and `AdsDDSetSubscriptionProperty` hold a user
  to `DROP` (of the subscription and its queue table) and `ALTER`
  (`fixtures/sql/ddapirights2`).
- A user's `EXECUTE` of functions, packages and procedures is checked in
  SQL: without the right the call is 5054 (a function's and a package's
  say "The user does not have permission to use the function/package.",
  a procedure's names it), and the calls a procedure or function makes in
  its body are not held to it (`fixtures/sql/ddexecrights`).
- A procedure, function, user, group, package, link, publication,
  subscription or database whose `DB:Public` rights slot does not check
  fails the system tables of its kind for any user but the administrator
  (5127) and the calls of a procedure, function or package in a dictionary
  that verifies rights (5054); `system.permissions` and
  `system.effectivepermissions` lose the rows that read the slot, fail
  where the original fails, and repeat rows as it does
  (`fixtures/sql/ddpublicbad3`, `ddpublicbad4`, `ddpublicbad5`).
- `sp_SetDDEncryptionType` converts a plain, RC4 or AES dictionary to RC4,
  AES-128 or AES-256 as the original does (`fixtures/aes/convert`): the
  key and the ADSSYS password stay, the users' passwords are lost when the
  cipher changes, the dictionary is changed in place or written anew
  packed from the first slot with `.add.BAK` and `.am.BAK` files, and one
  with an encrypted table is 5130. `sp_ModifyDatabase` sets
  `ENCRYPT_TABLE_PASSWORD` and `DATA_ENCRYPTION_TYPE` (3, 5, 6) of a
  dictionary of the default layout.
- AES dictionaries: full-text orders on tables whose index is enciphered,
  `ALTER TABLE` and `AdsRestructureTable` of encrypted tables (put in the
  clear, the old files kept as clear `.BAK` files), writes with an ADSSYS
  password, and `CREATE DATABASE … ENCRYPT TRUE` without a type (an RC4
  dictionary) match the original (`fixtures/aes/dictwrite`, `40_` to
  `45_`). A full-text order on any dictionary table is recorded in the
  dictionary with its options (it failed with 3515), and an enciphered
  index file that is written keeps the pages it did not change.
- Rights granted to `DB:Public` are read, written (API and SQL) and
  enforced as the original does, in the object's own enciphered slot with
  its checksum (`fixtures/dictionary/public.json`); every user holds them
  besides its own, and `AdsDDDeployDatabase` carries those on views,
  procedures, links and publications.
- Free tables on a dictionary connection and a dictionary table's missing
  files as the original does them (`fixtures/dictionary/freetables`):
  `CheckFreeTableAccess` (the connect option 0x2000 or the connect
  string's key) refuses free tables with 7087 to a user who is neither
  ADSSYS nor of `DB:Admin`; `AdsCreateIndex` by a user without `ALTER`
  makes a temporary index (deleted when the table closes; 5054 into the
  table's own index file), and by the others an index that joins the
  dictionary (5075 into the table's own file when not open exclusively);
  `Table_Auto_Create` makes a dictionary table's missing `.adt`, `.adi` and
  `.adm` again, and a missing file without it is 7041 (was 5004); the
  rights properties 1001 to 1005 are 5126 or 5042 when set.
- A user who is not the administrator renames, with `AdsDDRenameObject`,
  a function, package, procedure, view, table, link or group it holds
  `ALTER` on, the names checked first as in the original
  (`fixtures/sql/ddfuncrights`). Such a user's `CREATE TABLE` without `IN
  DATABASE` makes a free table (a primary key there is 5132), and with `IN
  DATABASE` a primary key works (`fixtures/sql/ddadminpk`). A user may
  insert, and update or delete without a condition, with no right to read
  the table; with a condition it is 5054 `Read permission not granted`
  (`fixtures/sql/ddinsertonly`). The grant option's refusals for
  functions, packages, links, users and groups, and rights on functions
  through a group, are pinned (`fixtures/sql/ddgrantopt2`, `ddfuncgroup`).
- Memo values and deletes in a transaction as the original's remote server
  does them (`fixtures/write/tx_memo_exclusive`, `tx_memo_sizes`,
  `tx_savepoint_lock`, `tx_savepoint_memo`, `tx_redelete`): an ADT table
  open exclusively holds back the values that need no new block; a
  rollback to a savepoint undoes newest first, and a commit after one that
  took back an appended record answers 7138; a record deleted in a
  transaction may be deleted or changed again.
- Full-text orders of FoxPro and VFP tables in a transaction take the
  commit's words in (`fixtures/write/fts_tx_cdx_*`, `fts_tx_vfp_*`; the old
  words were read after the memo values were overwritten), and a rollback
  gives a DBF table's memo blocks back newest first.
- Orders on expressions over NChar and NVarChar fields with `REPEAT` (a
  constant count), `DESCEND`, `VAL`, `SOUNDEX`, `BASE64ENCODE` and
  `COALESCE` with an 8-bit first argument are made and kept current as the
  original does (`fixtures/write/nchar_expr_more`); `BASE64ENCODE` keys are
  one byte longer, as the original's are, over 8-bit text too.
- `AdsSetFieldRaw` and `AdsSetTimeStampRaw` set a table field's bytes
  whatever its type, as the original does (`fixtures/errors/param_n.json`):
  cut at the field's length with 5071, 5066 on memo fields, 5070 on a GUID
  of another length than 16, AutoInc and RowVersion left as they are,
  short values padded or keeping the old bytes. A group's rights
  properties (1001 to 1004) list `name:rights;` for each table, view and
  procedure; an RI rule's properties give the original's texts and
  quirks; `AdsDDGetPermissions` of a group on a connection with no user
  is 5054.
- `AdsDDRenameObject` and `sp_RenameDDObject` rename publications and
  subscriptions (`fixtures/sql/spddrenamerepl`, `spddrenamerepl2`); what
  names a publication follows it, and a subscription's queue table keeps
  its name. Publication and subscription names are refused to every other
  kind of object but triggers (and the reverse), as in the original, and
  `AdsDDDeleteSubscription` of a renamed subscription deletes only the
  subscription, as the original does.
- `AdsSkip` from a record an ADT table was left on by a written delete or
  a cancelled append moves on from that record's slot (in an order, from
  where its key would be: skip n lands on the (2n - 1)th key), as the
  original does; the delete before its write acts the same
  (`fixtures/write/skip_deleted`).

- Full-text orders on ADT tables are kept current in a transaction as the
  original's remote server keeps them, byte for byte
  (`fixtures/write/fts_tx_*`): the orders hold no word of it until the
  commit, a rollback leaves them as they were, and the commit replays each
  write's words; a commit frees the memo values of the images a record had
  between, a deleted slot keeps its last image, and a rollback writes
  nothing of a change pending in a handle.
- Orders on expressions over NChar and NVarChar fields (`UPPER(nc)`,
  `nc + nv`, `SUBSTR`, `LEFT`, `RIGHT`, the trims and pads, `IIF`,
  `COALESCE`, `LEN`, ...) are made, kept current and sought as the
  original does (`fixtures/write/nchar_expr`); `UPPER` and `LOWER` of
  Unicode text map a character at a time by Unicode 5.1
  (`crates/tflip-storage/src/icu_case.txt`); a scope on a Unicode order
  ends with a 0.
- A dictionary with an administrator password is written as the original
  writes it: users, groups, rights, links and subscriptions take the key
  from the connection's login (`fixtures/sql/spddadminpw`).
  `sp_ModifyDatabase('ADMIN_PASSWORD')` and `AdsDDSetDatabaseProperty` 101
  set the password as the original does (by `ADSSYS` only, 5054 for anyone
  else; the key kept), and `AdsDDDeployDatabase` of a source with a
  password gives the deployed dictionary that password
  (`fixtures/dictionary/deploy`, `src_pw_*`). A `DB:Admin` member's grant
  or revoke clears the `Info2` flag only of the permissions granted to it.
- `LEAVE` and `CONTINUE` work inside the `WHILE` loops of scripts,
  procedures and triggers (nested, and in `IF`, `TRY`, `CATCH` and
  `FINALLY`); a stored body with a `LEAVE` outside a loop or statements
  after one is refused only when it runs (5154, 5177), as the original
  does (`fixtures/sql/loops.json`).
- A decimal literal goes into a NUMERIC column rounded half away from
  zero; string and double parameters with more decimals are cut, written,
  and fail with 2102; `SET` of a value cut to a variable is worded without
  its name; a parameter added to a date is a number of days; and a live
  `SELECT` checks its parameters when it starts
  (`fixtures/sql/gram_assign.json`; `fixtures/sql/claims.json`: 924 of
  944).
- The details of a refused property are the original's for groups, links,
  views, procedures, publications, articles and subscriptions, and for a
  dictionary connection as `ADSSYS` as for one with no user: 5054 for the
  properties only a user reads, the 5133 sizes of the numbers it may read,
  a group's rights properties (success with 5137 as the last error), and
  5132 for a missing article (`fixtures/errors/param_n.json`, 449 more
  rows; its dictionary is `fixtures/errors/param_n/indexed_table.add`).
- `AdsDDDeployDatabase` makes the folder's dictionary anew from the
  source's objects as the original does: tables first, then views, links,
  procedures, triggers, RI rules, publications, packages, functions, groups
  and users with their rights, each with a new id, what names another
  object or is enciphered with its id written again, and the log, version
  and database settings as the original's (`fixtures/dictionary/deploy`,
  `obj_user`, `x_*`); tables in subfolders are found, or made with their
  folders; and `usValidateOption` 2 takes the records that break a
  constraint out of the tables into fail tables, one constraint at a time.
  A source with an administrator password, subscriptions, rights granted
  to `DB:Public` or a field's message, and the move of a DBF table's or a
  memo table's records, fail with `AE_NOT_IMPLEMENTED`.
- `AdsStmtSetTablePassword` and `AdsStmtClearTablePasswords` apply to the
  tables a statement's SQL opens (5098 without the password, 7060 with a
  wrong one; `fixtures/write/encryption/stmtpw.json`), and
  `AdsStmtEnableEncryption`/`Disable` are kept as the original keeps them.
  `AdsDDCreate` with a non-zero `usEncrypt` makes an RC4 dictionary, its
  records, index and memo values enciphered as the original's
  (`fixtures/aes/dictwrite`, `22_rc4_create`). `sp_SetDDEncryptionType`
  refuses with 7078, 7068 and 7060 as the original does before it
  converts, and still does not convert.
- `sp_PackTableOnline`, `sp_ReindexOnline` and `sp_PackAllTablesOnline`
  run on `tflipd` as on the original's remote server
  (`fixtures/events/events_online_args.json`, `events_online_procs.json`):
  the table stays open to other connections, the final stage waits for
  the transactions on it up to the `Timeout` option (7176, 7189), a pack
  signals `__OPFinalStage_<table>` (one per table for a pack of all, none
  for a reindex), and the `Timeout`, `Indexes` and `MemoBlockSize` options
  and their errors (5019, 5058, 5065, 7041, 5021, 5020, 2129) are the
  original's; the offline pack and reindex give 7008 meanwhile, naming no
  file for a dictionary's table.
- SQL edge cases answer as the original does: NULL as a `CASE` result
  and a `CASE` of nothing but NULLs (2159), the `ELSE` location in the
  2191 of dates and timestamps, `NUMERIC` with more decimals than digits
  (5012), `ORDER BY EXPR` (2121) and a `UNION`'s `ORDER BY` of a column
  outside its first list (2145), a `{fn}` escape's errors at its
  parenthesis, an aggregate in a `DELETE`'s `WHERE`, the `=` of `IN
  (SELECT)`, `LEAVE` and `CONTINUE` outside a loop (2219), a logical in a
  `UNION` with or `INSERT`ed into an integer, a GUID with a RAW, `CHAR`
  with `NCHAR`, and casts to `SQL_TYPE_DATE`, `_TIME` and `_TIMESTAMP`
  (`fixtures/sql/claims.json`: 912 of 944).
- A failed `ALTER TABLE` of a dictionary's table with orders in several
  index files (`IN FILE`) leaves the files after the failing one
  unchanged, the failing file with the orders built before it, and the
  earlier files built (`fixtures/sql/spddalterfail3`).
- A dictionary's memo file after `CREATE TABLE` and `ALTER TABLE` of
  fields with long properties, and its slots after a failed restructure of
  a table with more orders than fields, are the original's
  (`fixtures/sql/spddlongalter2`, `spddlongalter3`, `spddalterfail2`).
- The dictionary getters word why they refuse an object or a property as
  the original does (5132 per kind of object, 5126, 5133, and 5054 for a
  connection with no user), and `AdsDDFindNextObject` and `AdsDDFindClose`
  refuse an unknown find handle with 5129. A statement's parameters,
  `AdsSetTimeStampRaw` and `AdsSetFieldRaw` refuse a pointer or a length
  as the original does (`fixtures/errors/param_n.json`).
- The `AdsMg*` calls refuse a bad handle, a NULL pointer and a size of 0
  in the original's order and words (`fixtures/errors/param_n_mg.json`).
- AES tables whose index is enciphered take `CREATE INDEX`, `DROP INDEX`,
  `sp_Reindex`, `sp_PackTable` and `sp_ZapTable` as the original does
  (`fixtures/aes/dictwrite`).
- Renaming a dictionary's DBF table (`AdsDDRenameObject`,
  `sp_RenameDDObject`) moves its memo file and every index file with it,
  as the original does (`fixtures/sql/spddrenamedbf`).
- A dictionary's encrypted table opens over `tflipd` through the
  dictionary (the server releases the table password from the login it
  holds), an AES dictionary can be connected to over `tflipd` (the
  `DDPassword` is sent with the connect), and `CheckFreeTableAccess` (the
  connect option 0x2000 and the connect string key) is sent to `tflipd`,
  which refuses free tables (7087) as the local server does. The protocol
  changes.
- `sp_GetSecurityInfo` gives the administrator of a dictionary that
  encrypts its records `DictionaryEncrypted` true, and keeps the encryption
  type the connection found at open; `AdsDDCreate101` refuses an unknown
  key of its connect string with 5019 and makes `Password` the new
  dictionary's ADSSYS password and `Description` its comment;
  `sp_ModifyDatabase` sets `DATA_ENCRYPTION_TYPE` and
  `ENCRYPT_TABLE_PASSWORD` on dictionaries of the AES layout too; the
  passwords of links and subscriptions in converted dictionaries match the
  original's (`fixtures/aes/convert`, `fixtures/aes/dictwrite`).
- A statement that reads the table it changes (`INSERT … SELECT`, a
  subquery in `INSERT` or `UPDATE`) is 7079 for a user who may change the
  table but not read it; through the table API, setting a field of a
  record that was there needs `UPDATE` on its column (5171), and a record
  just appended may be set and deleted with `INSERT` alone
  (`fixtures/sql/ddinsertonly2`).
- `GRANT ALL` and `REVOKE ALL` to `DB:Public` write nothing where the
  user's grantable rights are cut to nothing, and give a table's slot only
  what the user holds with the grant option on the whole table
  (`fixtures/sql/ddpublicgrant`).
- A table or view whose `DB:Public` rights slot does not check fails its
  open with 7207, its drop and the effective permissions with 5127, and
  the listings a user held to its rights sees with 5127; `system.columns`
  shows a user the columns of a table it reads through `DB:Public`
  (`fixtures/sql/ddpublicbad`, `ddpublicbad2`).
- The `AdsDD` calls that drop objects or set properties hold a user who
  is not the administrator to the right the original asks (`ALTER` or
  `DROP` on the object, its table or its publication; the administrator or
  `DB:Admin` for the database's properties) and answer 5054 and 5132 as
  themselves, in the original's order and words
  (`fixtures/sql/ddapirights`); `AdsDDDeleteIndex` takes the order out of
  its index file, and `AdsDDDeletePublication` takes the rights on the
  publication with it.
- A user, a publication, a trigger or an integrity rule is renamed by a
  user who holds `ALTER` on it (on its table for a trigger, on both tables
  for a rule, which then fails with 5138 for all but the administrator),
  and a member of `DB:Admin` renames anything
  (`fixtures/sql/ddrenamerights`).
- `CREATE TABLE` of a DBF table (CDX, NTX, VFP) by a user who is not the
  administrator is held to the right to create tables with `IN DATABASE`,
  is a free table without it, and gives the creator its rights before the
  key's objects (`fixtures/sql/ddcreatedbf`).
- RC4 dictionaries with a table password: `ENCRYPT_NEW_TABLE` and
  `ENCRYPT_INDEXES` can be set (5162 only without the password; the
  password is at least 6 characters, 5097, and cannot change while a table
  is encrypted, 5130), and the tables `CREATE TABLE`, `AdsCreateTable` and
  `AdsDDAddTable` make or add are encrypted, their indexes too with
  `ENCRYPT_INDEXES`, as the original does; `sp_EncryptTable`,
  `sp_DecryptTable`, the `TABLE_ENCRYPTION` property, restructures of
  encrypted tables and `AdsEncryptTable`/`AdsDecryptTable` of a
  dictionary's table follow the original (`fixtures/aes/rc4tp`).
  `AdsDDGetDatabaseProperty` of 106 and 120 and `AdsDDGetTableProperty` of
  214 report the real values, and `AdsCreateTable` on a dictionary
  connection adds the ADT table to the dictionary.
- A dictionary's ADT table no longer opens as a free table (5159), as in
  the original.
- A connection's temporary tables are kept in `tflip-temp/<pid>/` of the
  temporary folder, and on Linux the first temporary table of a process
  removes the folders of processes that have ended (a killed `tflipd`'s).
  The tests keep their files in `tflip-tests/<pid>/`, which the next run
  clears the same way: a run leaves only its own.
- Protocol version 88 (a statement's table passwords and encryption
  settings; 7138, a new error; the dictionary password of `connect_path`,
  `set_check_free_tables`, `set_encryption_type` and `set_table_type`).

## v0.25

- Full-text indexes are written, byte for byte as the original engine
  writes them (`fixtures/write/fts_build`, `fts_writes`, `fts_rebuild`):
  `CREATE INDEX … CONTENT` with `MIN WORD`, `MAX WORD`, `DELIMITERS`,
  `NOISE`, `DROPCHARS`, `CONDITIONALS` (each with `NEW`), `CASESENSITIVE`,
  `KEEPSCORE`, `NOTMAINTAINED` and `PROTECTNUMBERS`, and `AdsCreateFTSIndex`
  (no longer a stub), on character and memo fields, in a table's
  structural index or a file of its own: the order's options block, and a
  key for each distinct word of each record with its count if it keeps
  scores. Appends, changes and deletes keep full-text orders current
  outside transactions (5042 before); `AdsReindexFTS` (no longer a stub),
  `AdsReindex`, pack and zap rebuild them. Two defects of the original's
  rebuilds are in `docs/ADS_BUGS.md`.
- An ADI file rebuilt with other pages (`AdsReindex61`) is read again by
  the handles that write it; a write after it failed with 5088.
- A text parameter given to a GUID is read as one, as the original reads
  it (`fixtures/sql/guid.json`): `WHERE g = ?` and `UPDATE`/`DELETE … WHERE
  g = :id` find the row (no row before), and text that is no GUID fails
  with 5228.
- `NEWID()` and `NEWIDSTRING()` are random, with the version and variant
  where the original puts them; they were made from the clock and a
  counter, so two processes could make the same GUID.
- AES data dictionaries are read, as the original reads them
  (`fixtures/aes/dict`, made with known secrets by
  `tools/ace/crypto/aesdict.sh`): one that enciphers its records opens with
  its dictionary password (`DDPassword` in the connect string; 7068 without
  it, 7060 with a wrong one, 5042 before), and in any AES dictionary the
  passwords, group memberships and grants, which it enciphers with AES-CTR
  where others use RC4, are read, so its users log in and their rights
  apply. Its encrypted tables open with the dictionary password; without it
  a statement fails with 7068, as the original's; a connection without it
  opens one another connection of the process opened with it, as the
  original's do.
- AES data dictionaries are created and written as the original writes them
  (`fixtures/aes/dictwrite`, recorded by `tools/ace/crypto/aeswrite.sh`,
  replayed and compared deciphered): `CREATE DATABASE … ENCRYPTIONTYPE
  AES128 | AES256 DDPASSWORD p [ENCRYPT TRUE]` (2200 before), and every
  change to one: its records in the AES layout, enciphered under fresh IVs
  in one that encrypts them, its memo values and the memo file's slab
  records behind 8-byte IVs, its index enciphered, its secrets with AES-CTR,
  its tables laid out for AES and, after `ENCRYPT_NEW_TABLE` (5162 before),
  armed under its key. A process reserves a block of IVs in the header as
  the original does. The memo files of AES tables keep their slab records
  behind IVs too. PBKDF2's keys are kept by the process, so an AES table or
  dictionary opened again opens at once.
- The index of an RC4-encrypted dictionary is built anew from its records
  deciphered; it was built from their enciphered bytes.
- The pointers of the setters, the `AdsGet*W`/`AdsSet*W` forms, the cursor
  getters, the SQL text calls, the filter, key and scope calls and the
  dictionary getters are refused with the original's `Parameter N` (and
  `Table name`, `Field name`, `Parameter 2 is not a valid string.`) texts and
  in its order (`fixtures/errors/param_n`); a cursor's unknown column is 5113
  for `AdsGetFieldType` and `AdsGetFieldLength`; `AdsCachePrepareSQL` of NULL
  SQL fails; a NULL SQL text of `AdsExecuteSQLDirect` leaves the last error.
- SQL's edge cases of the grammar are read as the original reads them
  (`fixtures/sql/claims.json`, 31 more of its cases): a `SELECT` in
  parentheses before a `UNION`; `'`, `&` and form feed in names; the
  `--(*vendor…*)--` escape; `.T.`, `.F.`, `5.`, `1e`; `Datetime'…'` and
  `Binary'…'`; several hint words in braces (also after `UPDATE` and
  `DELETE`); `SHOW ALL INTERNAL PLAN`; `EXECUTE PRINT`/`DPRINT`, `ELIF`,
  `DBREAK`, `FOR` (2110); and the SQL and script reserved words refused as
  aliases and variable names (2115, 2217).
- `AdsDDRenameObject` (no longer a stub) renames tables, with their files
  and memo file unless `ADS_KEEP_TABLE_FILE_NAME`, functions, packages and
  every other kind `sp_RenameDDObject` renames, byte for byte
  (`fixtures/sql/spddrenameapi`), answering the original's plain ACE codes;
  fields, indexes and index files are refused with 5136, as the original
  refuses them. `sp_RenameDDObject` now also moves an ADT table's `.adm`,
  renames functions and packages, and takes a trigger's new name qualified
  by its table's.
- `AdsDDDeployDatabase` (no longer a stub) deploys a data dictionary to a
  folder of free tables as the original does (`fixtures/dictionary/deploy`,
  `tools/ace/dd/deploy.sh`): its tables are found by their relative paths,
  missing ones made, ones with other fields restructured (the old file kept
  as `.BAK`), missing orders built and orders the dictionary lacks
  dropped, the tables bound; the folder gets the dictionary's files, the
  source's are renamed `adsbackup`, with the `backup` folder and
  `adsdeploy.log`. Constraint validation, a source with an administrator
  password, and users, groups and the other objects fail with
  `AE_NOT_IMPLEMENTED`.
- `tflipd` answers an external procedure's library as the original's
  remote server does (`fixtures/sql/aeprulesremote.json`): its
  `AdsCommitTransaction` and `AdsRollbackTransaction` on the handle it is
  given end only the levels it began (5047 otherwise), a refused call
  spoils the caller's transaction (7207 for its later commit or rollback),
  and its own `AdsConnect60` is a connection of type 4 (types 1, 2 and 3:
  6082, 6097). `__output` no longer opens through the table API (7112, as
  on both of the original's servers), and `__input`, `__output` and
  `__error` by their plain names are 7041 (`aeptablesremote.json`).
- `INSERT`, `UPDATE` and `DELETE` by a user with no rights at all on the
  table fail with 7087, not 7079.
- SQL's edge cases of functions, numbers and types answer as the
  original's (`fixtures/sql/claims.json`, 105 more of its cases):
  `OSUBSTRING`, `OINDEXOF`; text read as dates in `YEAR`, `HOUR`,
  `TIMESTAMPDIFF`/`TIMESTAMPADD`; `EXTRACT(WEEK …)` refused; BIGINT wraps;
  MONEY rounds in arithmetic and truncates in casts, its range checked;
  NUMERIC results at most 32 wide; casts of text to numbers; the bitwise
  operators' operands; logical comparisons; `IN` lists of values only; the
  date limits; joining CICHARACTER with case-sensitive text (2214); the
  errors of `CASE`, `COALESCE` and `IIF` mixing types; and `GROUP BY` and
  `ORDER BY` of constants and ordinals.
- A user who is not the administrator is held to the grant option in
  `GRANT` and `REVOKE`, as the original holds it (`fixtures/sql/ddgrantopt`:
  5054 with its texts; `ALL` is cut to what it holds with the option; the
  right to create objects is never given `WITH GRANT`).
- Membership in the built-in groups (`DB:Admin`, `DB:Backup`, `DB:Debug`,
  `SERVER:Admin`, `SERVER:Monitor`) is written and read as the original
  keeps it (`fixtures/sql/ddbuiltin`): `sp_AddUserToGroup`,
  `sp_RemoveUserFromGroup`, `AdsDDAddUserToGroup` and
  `AdsDDRemoveUserFromGroup` (5042 before); a member of `DB:Admin` holds
  every right, and `system.usergroupmembers` lists the members.
- `AdsDDAddView`, `AdsDDAddProcedure`, `AdsDDAddTable`, `AdsDDCreateUser`,
  `AdsDDCreateUserGroup`, `AdsDDCreatePublication` and
  `AdsDDAddUserToGroup` refuse a user without the right with 5054, as the
  original's do, and give the user who adds an object every right on it.
- AES dictionaries are made by `AdsDDCreate101` too, from
  `EncryptionType=AES128|AES256` and `DDPassword` in its connect string,
  refused as the original refuses (5019, 5128, 5223, 5005, the half-made
  files of the last two left as it leaves them); `sp_EncryptTable`,
  `sp_DecryptTable` and the `TABLE_ENCRYPTION` property encrypt and
  decrypt an existing table of one, with its memo values and index files
  (`fixtures/aes/dictwrite`); `ENCRYPT_INDEXES` is kept (5162 before); the
  memo files of AES tables keep their free runs and values behind IVs; and
  the indexes of an encrypted table take writes. `AdsDDCreate` of a
  dictionary that is there fails with 5128.
- Full-text orders on Binary, Image and VarBinary fields, and on FoxPro and
  VFP tables (a tag of the structural `.cdx` for `CREATE INDEX … CONTENT`,
  a single-order `.idx` or `.cdx` file of its own for `AdsCreateFTSIndex`),
  are made and kept current as the original does, byte for byte
  (`fixtures/write/fts_binary`, `fts_cdx_create`, `fts_cdx`,
  `fts_cdx_null`), and recalls put their words back as the original's do
  (`fts_recall`, `fts_recall_binary`).
- `ALTER TABLE` of a dictionary's table with long column properties
  writes the dictionary's memo file as the original does
  (`fixtures/sql/spddlongalter`), and a failed restructure gives its
  objects back their record slots and keeps the orders built before the
  failing one as the original does (`fixtures/sql/spddalterfail`).
- `sp_CreateIndex90` on a dictionary's table, of a key expression and
  condition longer than 275 bytes together, panicked; the Key object goes
  on in the memo file as the original's does (`fixtures/sql/spddlongidx`).
- Protocol version 83 (`create_fts_index`, `reindex_fts`, `create_dictionary_connect`, the options of `rename_object`; 7068 and 7207, new errors).

## v0.24

- External procedures reach `__input`, `__output` and `__error` through
  the table API (`AdsOpenTable` of `#__input`, or of `__input` with
  `ADS_TEMP_TABLE`; `AdsAppendRecord`, `AdsSetField`, `AdsWriteRecord`), as
  Delphi procedures do: the first posted `__error` row is the error, in
  order with those SQL wrote (`fixtures/sql/aeptables.json`, recorded on
  the original's local server). `ADS_TEMP_TABLE` opens a connection's
  temporary table by name on a dictionary connection. `__error`'s columns
  are `errno` and `message`, as the original names them.
- Triggers can be functions of a library (`CREATE TRIGGER … FUNCTION f IN
  LIBRARY "x.aep"`, a DLL or a Linux shared object), in the local server and
  in `tflipd`, as the original's server calls them
  (`fixtures/sql/aeptrig.json`): with the help's seven arguments, `__new`
  and `__old` read through the handle (neither with `NO VALUES`, memos NULL
  with `NO MEMOS`), failing only through `__error` (5177 `TrigError n:`, or
  the message alone), the change and the library's writes rolled back
  (its writes kept with `NO TRANSACTION`), no `Startup` or `Shutdown`, and
  the original's texts for a function or library not there. `UPDATE __new`
  runs and changes nothing, as in a script.
- External procedures run, in the local server and in `tflipd` (5042
  before): the library a dictionary's procedure names, a Windows DLL or a
  Linux shared object beside the `.add`, is loaded and its version 2
  interface called as the original's server calls it: `Startup` once per
  connection, the procedure's function with the connection's id and handle,
  `Shutdown` when the connection closes. The library calls back through
  tflip's ACE, which it links, reading `__input` and writing `__output` and
  `__error` with SQL, inside the caller's transaction; the rows affected are
  the ones it says; its failures (an `__error` row numbered other than 0, a
  non-zero return, a function or library not there) are worded as the
  original's (`fixtures/sql/aep.json`, recorded with `tools/ace/aep/`). In
  `tflipd` the library's own copy of the client library sends its calls to
  the session of the client that runs the procedure, in the process
  (`tflip-aep`).
- A trigger body can call stored procedures (`EXECUTE PROCEDURE` in a
  trigger, 5042 before): arguments from the record, variables and
  subqueries, failures worded in the trigger's error as the original words
  them, the rows written counted; a text parameter of a stored procedure
  takes text only (2124 for a number) (`fixtures/sql/trigproc.json`).
- Triggers of one timing and priority fire in the original's order: newest
  first, but for the places of deleted records that a connection's copy of
  the dictionary keeps until a trigger fires; `system.triggers` lists by
  record (`fixtures/sql/trigorder.json`, `trigorder2.json`).
- A trigger script of exactly 65,535 bytes is stored as the original
  stores it (a length of 0, no bytes) and fails the statements of its event
  with 5005 (`fixtures/sql/trigbig.json`); triggers and the procedures they
  call nest 64 levels as the original counts them
  (`fixtures/sql/trigproc2.json`); errors that name a failed trigger are
  cut at 562 characters.
- Bookmarks are the original's: 8 bytes through a table, 12 and the key's
  length through an order, `AdsCompareBookmarks` ordering by key and record
  (reversed for a descending order), `AdsGotoBookmark60` through an order
  doing nothing, and a deleted record's bookmark failing with 5022 as
  `AdsGotoRecord` does (`fixtures/bookmarks/bookmarks.json`).
- A name that comes twice in SQL is two parameters, and `AdsGetNumParams`
  counts every marker, the most of any text the statement prepared
  (`fixtures/sql/params4.json`, `params5.json`).
- `AdsGetLastError` names the argument (`Parameter N`) of a NULL pointer
  or a refused option in about 90 getters, with the original's order of
  checks; `AdsGetKeyNum` and `AdsGetKeyCount` refuse options other than 1,
  2 and 3, and `AdsGetDeleted` and `AdsGetVersion` NULL pointers
  (`fixtures/errors/param_n.json`). The protocol is at version 80.
- System tables and catalog procedures show a user only what its rights
  allow, with the original's masks: the tables, columns, indexes, views,
  procedures and functions it holds a right on, `system.dictionary` in
  part, no database triggers or integrity rules; `Triggers_Disabled` shows
  the database's flag; `sp_ModifyDatabase` takes `LOGINS_DISABLED_MSG` and
  `FTS_NOISE_WORDS` (`fixtures/sql/ddvisible`).
- A dictionary whose logins are disabled refuses every user but `ADSSYS`
  with 7109 (and the database's message), and a user whose own logins are
  disabled with 7113 (`fixtures/sql/ddlogins`).
- `sp_DisableTriggers` and `sp_EnableTriggers` work for a user who may
  alter the table, as the original's do, not only for the administrator
  (`fixtures/sql/ddtrigrights`).
- Column rights as the original enforces them: NULL for a column the user
  may not read, 5171 for a condition on one (permission levels 2 and 3) and
  for an `UPDATE` of a column without the update right, level 3 for SQL
  alone (static cursors, 7086 on `AdsOpenTable`), 7087 for an `INSERT` or
  `UPDATE` without the right; a restricted user's open options drop the
  permissions it lacks (`fixtures/sql/ddcolrights`).
- `AdsConnect101` refuses the keys and values the original refuses (5019
  with the key or value added) and acts on `TableType`, `CharType`,
  `ReadOnly`, `Shared`, `LockMode`, `DateFormat`, `Exact`, `Collation` and
  `TrimTrailingSpaces`; `AdsOpenTable101` opens tables as they say
  (`fixtures/config/connect101*.json`).
- `ADS_REINDEX_ON_COLLATION_MISMATCH` rebuilds an index of another
  collation on opening, byte for byte as the original does, and
  `ADS_IGNORE_COLLATION_MISMATCH` opens it as it is; a table with a
  collation over an uncollated index is 5209
  (`fixtures/collation/mismatch.json`).
- `ADS_UCHAR_KEY_SHORT`, `_LONG` and `_XLONG` size the keys of NChar and
  NVarChar orders (2, 4 and 5 bytes a character), and `AdsBuildRawKey100`
  takes `ADS_GET_PRIMARY_WEIGHT_LENGTH` and refuses other options than 0,
  1, 2 and 4 (5019); an ADI order with a Unicode field has pad byte 0, and
  an index option outside the mask is refused with 5019
  (`fixtures/write/uchar_keys`).
- 944 SQL edge cases of the original are replayed
  (`fixtures/sql/claims.json`, `claims_huge.json`, the latter ignored as
  slow), 183 of them known to differ. Of what they found: `SET variable =
  expression` assigns in a script; joining fixed-length Unicode texts past
  32,500 characters is 2119 and variable-length ones cap at 32,500 (a debug
  build used to panic); `LEFT`, `RIGHT`, `SUBSTRING`, `REPEAT`, `REPLACE`,
  `SPACE`, `STR` and `DTOC` refuse the arguments the original refuses
  (2159), `TRUNCATE` takes two arguments and `CHAR(n)` wraps at 256; a
  `LIKE … ESCAPE` of more than one character, `%` or `_` is 2115, as are
  `x NOT = y` and `- -x`; a result column name over 128 characters, or one
  that cannot be made unique, is 5012, and a name or number over 1024
  characters 2114.
- `system.dictionary` reports `Dictionary_Encrypted`, `Encrypt_New_Table`,
  `Encrypt_Indexes` and `Encrypt_Table_Password` (the password, padded to
  20 characters) as the original does (`fixtures/sql/ddenc.json`).
- `CROSS JOIN` is refused as the original refuses it: `CROSS` is read as an
  alias, so the `JOIN` wants its `ON` (2115, or 2117 after an alias or an
  `ON` condition). Joins without an `ON` take the joins after them as their
  right side, an error in a subselect carries the messages of the clauses
  around it, and a syntax error in a select-list subselect is reported as
  `identifier or expression` at its first word
  (`fixtures/sql/gram_cross.json`).
- Update the DLL and `tflipd` together: the collation-mismatch and
  Unicode-key options travel as bits of the existing open and index-creation
  requests, which an older `tflipd` ignores without an error.
- Online maintenance through `tflipd`: `CREATE INDEX` and `ALTER TABLE`
  close the table to other connections (7008), `CREATE ONLINE INDEX` and
  `ALTER ONLINE TABLE` leave it open and signal `__OCIFinalStage_<table>`
  and `__OAFinalStage_<table>` at their final stage, and `ALTER
  ONLINE_WITH_TRANSITION TABLE` waits for the connections that opened the
  table meanwhile (`fixtures/events/events_online.json`,
  `events_online_final.json`).
- An index tree whose second level filled as its levels ended panicked
  (`CREATE INDEX` of three fields on 131,072 records); it is built.
- A setter that fails leaves an update in progress, or none, as the
  original does by the kind of failure (`fixtures/write/set_errors`);
  `AdsCancelUpdate` on no record is 5068, not 5085.
- A deleted record the cursor was left on takes a field set and a delete in
  a table open exclusively, as the original does: it is deleted again when
  written, with the free list and the binary order's change count as the
  original keeps them, and `AdsGetRecordCount` one lower than the file's
  header until an append (`fixtures/write/deleted_cursor`).
- Query logging records the statements that triggers, stored procedures
  and multi-statement scripts run, each as a row of its own as the original
  does: the text as written, unoptimized, the rows changed so far in the
  script, the end time of a procedure's last statement; a script is no row
  of its own (`fixtures/sql/spddql3.json`, `spddql4.json`). A stored
  procedure with no output parameters and no `CURSOR VARYING` returns no
  cursor, and the error of a failed nested procedure names the procedure it
  failed in.
- `AdsSetBinary` in pieces (`ulTotalLength`, `ulOffset`) on ADT Binary,
  Image and VarBinary fields: padding, cuts, NULL, 5082 for a bad piece,
  5066 and 5070 for a wrong binary type, 5227 for a compressed field
  (`fixtures/write/binary_pieces*`).
- `AdsGetField` with `ADS_GET_GUID_MIME`, `_FILE`, `_NUMBERS` and
  `_REGISTRY` (5019 for more than one; `fixtures/write/guid_formats*`).
- `AdsGetHandleType` gives 6 for a dictionary connection and 8 for a
  full-text index order (`fixtures/write/handle_kinds`).
- `AdsCopyTable`, `AdsCopyTableContents` and a same-type `AdsConvertTable`
  of a table opened exclusively leave `AE_INFO_COPY_MADE_BY_CLIENT` (5169)
  as the last error, returning 0 (`fixtures/write/copy_made_by_client`).
- Cursors in scripts over a join with no equality (a cross join, `ON t.n <
  u.g`), a `RIGHT JOIN`, a `LEFT JOIN` with a condition beyond the equality
  in its `ON`, a `SELECT` of columns in `FROM` that is joined or sorted, and
  a join of three tables with an index on a linking column or a condition
  on the second or third table read their records live as the original
  does, instead of at `OPEN` (`fixtures/sql/cursormore5.json`, replayed).
- An ADI or CDX file takes 256 tags, as the original's (5046 beyond): its
  tag directory grows past one page, splitting like an order's tree, and a
  tag's root leaf is written after the pages the directory took
  (`fixtures/write/tagdir`, `tagdir_drop`); dropping tags from a directory
  of several pages matches too.
- `sp_GetSQLStatements` on `tflipd` lists every client's statements on
  the dictionary, with `Active`, and `sp_CancelQuery` finds another
  connection's wait; `tflipd` connections see the dictionary changes other
  connections made; a second handle cannot lock or unlock a record a
  transaction wrote through the first (`fixtures/events/events_statements.json`,
  `fixtures/write/tx_handles`).
- `ALTER TABLE` of a dictionary's Clipper, FoxPro and Visual FoxPro tables
  that changes fields restructures the table, or renames the fields in
  place, as the original does; `CREATE INDEX` on a dictionary's DBF table
  and `sp_CreateIndex90` make Key objects, and Key types follow retyped
  fields; an order that cannot be built is undone as the original undoes
  it (3015); `ALTER TABLE`'s errors show the full field definition for
  every table type; a dictionary table whose listed index file is missing
  no longer opens (7041) (`fixtures/sql/ddalterdbf`, `ddalterfail`,
  `ddalterfree`).
- The dictionary's memo file is written byte for byte as the original
  writes it: the properties of procedures, views, triggers, functions and
  new tables' columns are set one at a time (allocation counts and stale
  bytes), and removing an object frees its `More Property` memo block and
  its comment's (`fixtures/dictionary/removelong`, `fixtures/sql/spddlong`).
- Database triggers on `OPEN_TABLE` and `CLOSE_TABLE` fire as the
  original's: each table a statement opens, once, closed when the statement
  runs other SQL, closes or its connection disconnects; the open count
  shared by connections; a client's cursor open closing with the cursor
  (5177 from a failing trigger); a running database trigger's own opens no
  longer fire its kind again; `UserID` and `StmtCount` of `CONNECT` and
  `DISCONNECT` as the original's; database triggers of one priority run the
  newest first (`fixtures/sql/trigdblog*.json`).
- `SELECT`: a lone `>=` or `<=` orders its rows by its column even with no
  order, and by the key of an order on several fields; orders on several
  fields, CICHARACTER ranges and collated ranges serve plans; `IN` lists are
  scanned as in the original; the smaller table of an inner join of two
  tables is read first (`fixtures/sql/indexsel*.json`, `joinorder.json`,
  `joinsize.json`).
- Syntax errors of `INSERT`, `UPDATE` and `MERGE` clauses, a call's
  missing `)`, `DATE '…'`, `!=`, `TOP 1.5` and `= SOME (…)` are worded and
  placed as the original's; `CAST` sizes wrap at 16 bits and stop at 65530
  (32500 for the wide types); `HAVING` without `GROUP BY` is 2196; a static
  cursor leaves out the later rows too large for their column
  (`fixtures/sql/gram_syntax.json`).
- A `SELECT` of one table in `FROM` is a live cursor on that table, with
  writes, as in the original (`fixtures/sql/derived.json`); an `ORDER BY` in
  a derived table or a `WHERE` subquery without `TOP` is refused (2115); a
  `COUNT(*)` alone and a derived table's `SELECT` show no scan in plans; a
  live cursor of fewer or reordered columns no longer fails its writes with
  5088 when the table has an ADI index.
- Script cursors over a `LEFT JOIN`, a join of three tables or more, a join
  with an `ORDER BY` or a condition on one table, `UNION ALL` and a `SELECT`
  in `FROM` read their tables live (`fixtures/sql/cursormore4.json`);
  `ORDER BY` on a `UNION` is refused in a script cursor (2145).
- Trigger scripts are refused at 65,534 bytes (5127) and above 65,535
  (5021), function scripts from 65,534 bytes (5133); a dictionary field's
  `NOT NULL` is checked before `BEFORE` and `INSTEAD OF` triggers run
  (5147), and `UPDATE __new` in a trigger is accepted and ignored
  (`fixtures/sql/limits_procs.json`, `trignn.json`, `txtrig3.json`).
- `SELECT … INTO #name` makes the connection's temporary table in the
  statement's table type (Clipper, FoxPro and VFP too), with the fields,
  refusals and name limits of the original's DBF tables; a Clipper or FoxPro
  temporary table opens through `AdsOpenTable` as either DBF type until a
  statement has opened it (`fixtures/sql/temptables_into*.json`,
  `temptables_api2.json`, `temptables_api3.json`).
- Event notifications (`sp_CreateEvent`, `sp_SignalEvent`,
  `sp_WaitForEvent`, `sp_WaitForAnyEvent`, `sp_DropEvent`,
  `sp_DropAllEvents`) across the connections of a process and the sessions
  of `tflipd`, as the original's remote server runs them
  (`fixtures/events/`); an `AFTER INSERT` trigger's rollback to its own
  savepoint takes back the insert, and a `NO TRANSACTION` trigger is a call
  (7129, 5047); a `tflipd` session's `sp_mgGetConfigInfo` shows the remote
  server's values.
- Smaller SQL answers as the original's (`fixtures/sql/gram_cast*.json`,
  `params3.json`, `planupd.json`, `spddri2.json`, `ddprocs2`): numbers,
  text and bytes where a logical is wanted; `CAST`'s type names (only
  `SQL_…`) and sizes, bytes cast to numbers; parameter text converted to
  numbers and dates; `VARCHARFOX` and `VARBINARYFOX`; the `RESTRICT`
  refusal's own text, and an integrity rule checked against the rows
  already there (5139); `SHOW PLAN FOR UPDATE`, `DELETE` and `INSERT`.
- Every object added to a dictionary takes a deleted one's slot, as the
  original does (`fixtures/sql/ddslots`); a user who is not the
  administrator is refused creating, altering and dropping views,
  procedures, tables, indexes, triggers, users, groups, links and integrity
  rules without the right, with the original's messages, and holds all
  rights on what it creates (`fixtures/sql/ddrights`); dropping a table,
  user, group or link takes its triggers and rights with it; a VARCHAR,
  VARBINARY or NVARCHAR field is recorded with its stored length.
- Query logging (`sp_EnableQueryLogging`, `sp_DisableQueryLogging`,
  `sp_GetQueryLoggingResults`, `sp_ViewQueryLogging`) as the original logs
  a connection's statements, and `sp_GetUnicodeConverters`,
  `sp_mgGetCacheInfo`, `sp_mgGetTableCacheInfo`, `sp_mgGetConfigMemory90`
  and `sp_mgKillUser90`.
- Writable SQL cursors: a live cursor, through a view too, takes appends,
  deletes, recalls, writes and record locks into its table as the
  original's does (`fixtures/write/cursor_*`), its `ORDER BY` following;
  `AdsStmtConstrainUpdates` refuses a write that leaves the `WHERE` (7065).
  The protocol is at version 79.
- A script's cursor sorted by an indexed column, and one over an inner join
  of two tables on one equality, see the changes made after `OPEN` as the
  original's do (`fixtures/sql/cursormore2.json`, `cursormore3.json`).
- `ALTER TABLE` of a dictionary's ADT table adds, changes and drops columns
  and their constraints as the original does, the dictionary byte for byte
  (`fixtures/sql/ddalter`); fields only named again are renamed in place
  without a restructure (free tables and `AdsRestructureTable` too); the
  5012 of a duplicate column shows its definition.
- Triggers: a trigger's `__error` table fails the statement with the error
  it holds (5177, `TrigError n:`); database triggers (`CREATE TRIGGER … ON
  DATABASE` on `CONNECT`, `DISCONNECT`, `OPEN_TABLE`, `CLOSE_TABLE`), the
  dictionary byte for byte, a failing `CONNECT` trigger refusing the login
  with 7109 (the administrator exempt) and a failing `OPEN_TABLE` one the
  open; `ALTER TRIGGER`; an `AFTER` or `INSTEAD OF` trigger's writes to its
  own table fire none of that event's triggers; a trigger's name need only
  be unique on its table.
- Transactions in SQL as the original's remote server runs them
  (`fixtures/sql/txscripts.json`, `txprocs.json`, `txtrig.json`,
  `txddl.json`): a failing script takes back the transaction it began; a
  procedure or transactional trigger is a call level, with its own
  savepoints, committing what it left open; `NO TRANSACTION` on one of an
  event's triggers applies to all; a cascading RI rule cannot lock a child
  the caller's transaction wrote (5035); `ALTER TABLE` that restructures
  and `DROP INDEX` are refused in a transaction (5063). `tflipd` enforces
  the statement limit (7216) and gives `::conn.NetworkAddress` and
  `sp_GetSecurityInfo`'s `TCP_IP`. The protocol is at version 78.
- A SQL `WHERE` run as a filter evaluates every operand of `AND`, `OR`,
  `IIF` and `COALESCE`, and fails with 4120 on a division by zero, as the
  original's expression engine does; `CAST` to text is as wide as the
  original makes it for `BIGINT`, `TINYINT`, `REAL`, memos and `MONEY`;
  `TIMESTAMP` takes no arithmetic (2124); a static cursor read through an
  order of its table has no index of its own; parameters in `CASE`, the
  results of `IIF`, signs and `NOT` are refused as the original refuses
  them.
- Binary literals (`x'2A 2B'`), the shifts `<<` and `>>`, the ODBC escape
  `{OJ …}`, the script forms `ELSEIF`, `ENDIF`, `ENDWHILE`, `ENDTRY`, `END
  TRY` and a bare `END`, `CHAR_LENGTH(x USING …)`, `SET
  ::stmt.unicode_sort_key_size` and `::conn.TerminalClientAddress`, as the
  original reads them (`fixtures/sql/gram_*.json`); `OCTET_LENGTH` and
  `BIT_LENGTH` count bytes, and a Raw column takes a binary literal and
  compares it byte for byte.
- `CREATE`, `ALTER` and `DROP FUNCTION` and `CREATE` and `DROP PACKAGE` on
  a dictionary connection, the dictionary byte for byte as the original
  writes it (`fixtures/sql/ddfuncs`), with its refusals and the rights a
  user who is not the administrator needs; `GRANT` takes a deleted
  object's slot, and `system.functions` lists a package's functions after
  the database's.
- Cursors in procedure and trigger bodies, over a `SELECT` and over
  `EXECUTE PROCEDURE`; a script's cursor over one table reads it live, as
  the original's does (updates, deletes and, unsorted, inserts after `OPEN`
  are seen); a temporary table made as another type is refused (7016, 7028)
  as the original refuses it.
- The catalog procedures the ODBC and JDBC drivers call (`sp_GetCatalogs`,
  `sp_GetSchemas`, `sp_GetTableTypes`, `sp_GetTypeInfo`, `sp_GetProcedures`,
  `sp_GetProcedureColumns`, `sp_SupportsConvert`,
  `sp_SupportsTxnIsolationLevel`, `sp_RetrieveSystemProcProperty`),
  `sp_GetCollations`, `sp_GetCollationTable` and `system.systemprocedures`;
  a dictionary user who is not the administrator is refused
  `sp_mgKillUser`, `sp_GetSQLStatements` and `sp_CancelQuery`.
- `UPDATE … FROM` with joins, and `UPDATE … WITH DELETE | WITH RECALL`, as
  the original runs them (`fixtures/sql/updfrom.json`, the files byte for
  byte): each record updated once from the first row of its join, the
  target found by name or alias, 2206 and 2132 for ambiguous ones.

## v0.23

- The original's limits are checked as it checks them, by table type and
  through the API and SQL alike (`fixtures/write/limits`,
  `fixtures/sql/limits.json`, `limits_procs.json`): field lengths and
  decimals, record length, memo block sizes, field, tag and table names,
  key and expression lengths, 15 open NTX files, 256 tags a file, 15 key
  columns in SQL, a procedure's script (5127, 5133) and 64 nested
  procedures and triggers (7093). An ADI file grows its pages for an order
  whose key needs them, or refuses it with 5188 where the page size was
  given. `Short` as a field type is a ShortInt, a legacy ADT `VarChar` makes
  its memo file, and an `ORDER BY` of 21 items or more gives a static
  cursor. The protocol is at version 77: `AdsReindex61`'s page size, the
  parse-only property and the expression engine's division errors.
- `ALTER TABLE … MEMOBLOCKSIZE` rewrites the memo file with the new block
  size, on free and dictionary tables, as the original does. `STOTS` reads
  the time after the date (`fixtures/expr/expressions.json`), so an SQL
  equality on an indexed TimeStamp finds its rows. A CR LF counts as one
  character in a syntax error's place. `AdsGetAllLocks` sets `*len` to the
  number of locks held when its array is short, `AdsGetAllTables` needs a
  slot more than there are tables, and `AdsGetBinary` fails with 5005 for a
  length of 0 and 5082 for an offset past the end, as in the original.
- ADT deletes and recalls write what the original writes: the binary
  orders' change counts in the tag headers (a recall counts in a binary
  order only, as does a cancelled append; `fixtures/write/binary_changes`),
  `AdsRecallRecord` on a deleted record recalls it at once, and an append
  takes the free list's head only while that record is still deleted
  (`fixtures/write/append_reuse`). `AdsReindex61` writes its page size
  (512 to 8192, 5019 otherwise; `fixtures/write/reindex_pages`), and a zap
  restarts the AutoInc counter at 1 (`fixtures/write/autoinc_reset`). A
  deleted ADT record cannot be locked (5035).
- Malformed `CREATE TABLE`, `CREATE INDEX`, `ALTER TABLE`, `DROP TABLE` and
  `DROP INDEX` fail with the original's codes, texts and places
  (`fixtures/sql/refuse.json`); keywords are refused as column and index
  names, and a trailing comma in `CREATE TABLE`, `ALTER TABLE … DROP
  PRIMARY KEY` and `ALTER COLUMN … DROP DEFAULT` are accepted, as in the
  original. `%` stays on the level of `*` and `/`, which the probe shows
  the original does too, though the help ranks it lower
  (`fixtures/sql/modulo.json`).
- `CREATE PROCEDURE` and `AdsDDAddProcedure` with parameter lists over 275
  bytes no longer panic: they continue in `More Property` as the original's
  do (`fixtures/dictionary/proclong`, `fixtures/sql/ddproclong`).
  `AdsCreateTable` and `AdsOpenTable` take a `#name` as the connection's
  temporary table, and `AdsOpenTable` refuses another table type than it
  was made as (7016, 7028; `fixtures/sql/temptables_api.json`).
- Text a script, a subquery, a derived table or a variable reads out of
  OEM tables is ANSI, and `ASCII` of it the ANSI code, as in the original
  (`fixtures/sql/oemrows.json`); the rows of a `LIKE` that is the only
  condition come ordered by its column (`fixtures/sql/likeorder.json`); and
  which scalar functions in a `WHERE` make a cursor static is recorded
  from the original (`fixtures/sql/statics.json`), which tflip already
  matched.
- SQL reads `TIME'…'`, `DATE'…'` and `TIMESTAMP'…'` literals and the
  `{d}`, `{t}` and `{ts}` escapes by the original's rules (a 12-hour time,
  a `T` between date and time), refuses text that is no date, time,
  timestamp or GUID where one is assigned or compared (2124), takes GUID
  text into a GUID column, truncates `CURRENT_TIMESTAMP(n)` to its
  precision, checks the unary `+` as the `-`, sorts a `UNION` by the first
  `SELECT`'s names, refuses a memo in `DISTINCT` (2194) and a system table
  or subquery on the right of a `RIGHT` or `FULL JOIN`, as the original
  does (`fixtures/sql/literals.json`). `INLIST` skips a NULL among its
  candidates.
- `IIF`, `IF` and `COALESCE` in the expression engine evaluate every
  argument, as the original does, and a division by 0 fails an index build
  (4020, a condition 4220, no file left behind), an AOF (4020), a filter's
  move (4120) and the write of a record (4020) instead of giving an empty
  key or a false condition. SQL's `IFNULL` evaluates both arguments
  (`fixtures/sql/iif.json`). `TRANSFORM` does pictures with the literals
  `%`, `-`, `/`, `:` and the options `B` and `R`, losing the last digit as
  the original does.
- Parameters nothing types fail as they do in the original (2154 for `- ?`
  and `NOT ?`, 2123 for `? IS NULL` and untyped operands, `COALESCE`'s
  2159), a character parameter is `VARCHAR(1024)` (2155 when a longer one
  is joined), and a re-prepared statement keeps its parameter values by
  position until `AdsClearSQLParams` (`fixtures/sql/params.json`).
- `AdsSetProperty` handles the ids ARC sets: 39 makes the statement's next
  `AdsVerifySQL` check the syntax alone, so ARC's "Verify syntax" passes on
  a procedure body that names `__input` and `__output`
  (`fixtures/sql/props39.json`); 87 answers that strong encryption is
  available, as the original with its AES DLLs does; 27 is accepted.
- Reindexing a FoxPro table leaves a custom order empty, as the original
  does, instead of filling it; `AdsAddCustomKey` and `AdsDeleteCustomKey`
  work on FoxPro orders; a reindex of an ADT table writes into each tag's
  header the count of the writes that changed that order; a candidate
  order over a NULL value is refused with 7153; and restructuring a FoxPro
  or ADT table with a custom order is refused with 5152, as the original
  refuses it (`fixtures/write/cdx_custom_reindex`).
- The dictionary system procedures (`sp_CreateUser` and the others) give
  their `?` and `:name` parameters the declared types, as the original does
  (`fixtures/sql/spddparam.json`). A system procedure's INTEGER parameter
  cuts a number to a whole one (`6.5` is 6) rather than refusing it, and a
  BOOL parameter given as a literal takes only 0 and 1
  (`fixtures/sql/spddnum.json`).
- `AdsDDSetObjectAccessRights` words 5137 (an unknown object) and 5158 (a
  right the object cannot carry) as the original does.
- A script variable in a trigger body is typed where the body's statements
  read it, as in every other script: a NULL `CHAR` variable no longer fails
  with "Data type of parameter cannot be determined"
  (`fixtures/sql/trigtyped.json`).
- `CAST(x AS SQL_CHAR)` without a length is as wide as the original makes it
  for `NUMERIC`, `DOUBLE`, `MONEY`, `SMALLINT`, `TIME` and the binary types
  (it was one character), a cast to `CHAR` is padded, a `NUMERIC` or `MONEY`
  below one has no leading zero, and a `TIME` or `TIMESTAMP` casts to its
  text (`fixtures/sql/castchar.json`).
- **The dictionary system procedures run**, writing the dictionary byte
  for byte as the original does (`fixtures/sql/spdd*`): `sp_CreateUser`,
  `sp_DropUser`, `sp_CreateGroup`, `sp_DropGroup`, `sp_AddUserToGroup`,
  `sp_RemoveUserFromGroup`, `sp_ChangeCurrentUserPassword`,
  `sp_CreateReferentialIntegrity`, `sp_DropReferentialIntegrity`,
  `sp_CreateLink`, `sp_ModifyLink`, `sp_DropLink`, `sp_GetLinks` (5054, as
  the original's), `sp_GetSecurityInfo`, `sp_DisableTriggers`,
  `sp_EnableTriggers`, `sp_ModifyDatabase` and
  `sp_Modify{Table,Field,Index,View,Procedure,User,Group,Trigger}Property`,
  `sp_AddTableToDatabase`, `sp_AddIndexFileToDatabase`,
  `sp_RemoveIndexFile`, `sp_RenameDDObject` and `sp_MoveDDObjectFile`;
  `sp_SetDDEncryptionType` checks its arguments as the original does but
  does not encrypt.
- Disabled triggers (of the table, view, trigger, dictionary or connection)
  no longer fire. A link made with an empty password has no password slot;
  a table moved with `AdsDDMoveObjectFile` names its dictionary from where
  it is, and a relative destination is stored as given; a removed index
  file's comment is freed; a dictionary password is cut to 20 bytes at
  login; `system.usergroups` shows a group's `User_Defined_Prop`.
- **The script host: variables, `__input`, subqueries and functions that
  run SQL.** `DECLARE` checks a type as the original does (2135, 2136,
  2217), has `ROWVERSION` and `MODTIME`, and assigns by its rules (5179,
  2227, 2102, text cut and padded); a variable stands in a statement as a
  value of its type. The system variables `::conn.*` (`Name`,
  `OperationCount`, `OSUserLoginName`, `ClientHostName`, `NetworkAddress`,
  `IsRoot`, `Collation`, `TransactionCount`), `::stmt.*` (`Name`,
  `UpdateCount` and the `Trig…` ones) and `::server.OldestActiveTxn` are
  read in statements, scripts, triggers, procedures and functions. A
  procedure reads its input as `__input`, runs `EXECUTE PROCEDURE`, checks
  its arguments as the original does, and fails as 5154. Subqueries run in
  a script's own expressions. A user-defined function's statements run on
  the calling statement's connection, a call with the wrong arguments is
  refused (2136, 2159), and a `WHERE` that calls one gives a static cursor
  (`fixtures/sql/declare.json`, `sysvars.json`, `trigvars.json`,
  `procvars.json`, `subq.json`, `udf.json`).
- **The table and management system procedures run.** `sp_PackTable`
  (with memo block sizes), `sp_ZapTable`, `sp_Reindex` (with page sizes),
  `sp_EncryptTable`, `sp_DecryptTable`, `sp_GetTableEncryptionType`,
  `sp_GetTableSizeInfo`, `sp_GetRecordCRC` and `sp_IgnoreTableTransactions`,
  on free ADT and DBF tables and on a dictionary's (which records the memo
  block size, the index page size and the transaction-free flag), with the
  original's argument checks (2141, 2129, 2136, 2124, 5179); the `…Online`
  forms refuse as the local server does (5042). The management procedures:
  `sp_mgGet…` (server type, activity, communication statistics,
  configuration and its memory, usage, users, open tables and indexes,
  locks, worker threads), `sp_mgKillUser`, `sp_mgResetCommStats`,
  `sp_SetStatementLimit`, `sp_SetRequestPriority` and `sp_CancelQuery`; the
  three only the root dictionary's administrator may run fail 5054
  (`fixtures/sql/spt*`, `spm*`).
- `AdsGetRecordCRC` checksums a DBF record without its 1-byte prefix, and
  an encrypted table's opened without its password over the raw record; a
  memo written after `AdsZapTable` no longer panics, and an ADT memo file's
  first free block is rounded up past the header when the block size does
  not divide 4096.
- **Cursors and `TRY … CATCH` in scripts.** `DECLARE c CURSOR [AS SELECT …
  | AS EXECUTE PROCEDURE …]`, `OPEN [AS …]`, `FETCH`, `CLOSE`, `WHILE` and
  `IF [NOT] FETCH` and `c.column` run, with the original's errors for a
  cursor closed, open twice or read past its end
  (`fixtures/sql/cursors.json`); `TRY … CATCH [ALL | name] … FINALLY … END
  TRY` runs, with `__errcode`, `__errtext`, `__errclass` and `RAISE;`
  passing the error on (`fixtures/sql/trycatch.json`). A `RAISE` with a code
  that is no integer or a message that is no text fails with 2124, and an
  assignment to an undeclared variable in a script sent as a statement at
  parse time, as in the original.
- **SQL text meets OEM tables in the OEM code page.** In a statement on
  OEM tables (Clipper and FoxPro tables opened with `ADS_OEM`), text in the
  `WHERE`, expressions, `INSERT`, `UPDATE` and `MERGE` and text parameters
  are converted from the client's code page by the best-fit table writes
  use, and the static cursor of such a `SELECT` hands its text out converted
  back to ANSI (`fixtures/sql/oem.json`).
- **`CACHE PREPARE` runs, and the original's syntax errors for DDL it has
  no statement of.** `CACHE PREPARE ON | OFF | DEFAULT` runs as a script
  statement (it changes nothing observable, as in the original), with the
  original's syntax errors. `CREATE`, `DROP` and `ALTER` of a word the
  original has no statement of (`CREATE USER`, `DROP DATABASE`, `ALTER VIEW`,
  …) fail with its 2117 and 2115 errors, placed as it places them, no longer
  with 2200; the `FUNCTION` and `PACKAGE` statements and `ALTER TRIGGER`,
  which it has and tflip does not run, are 2200 by name
  (`fixtures/sql/refuse.json`).
- **`CREATE VIEW`, `DROP VIEW`, `CREATE`, `ALTER` and `DROP PROCEDURE` and
  `CREATE DATABASE` run.** A view takes the statement the original keeps
  (from its first word to its last token, without the comments and the `;`
  around it), its refusals (the name of a view, a table or a procedure, a
  `SELECT` that does not read, an `ORDER BY` without `TOP`, parameters) and
  the slot of a deleted object; a procedure, script or external (`FUNCTION f
  IN [COM]LIBRARY lib`), with `OUTPUT` and `CURSOR VARYING OUTPUT`
  parameters, is written as the original writes it, `ALTER` in place so that
  its permissions stay and `DROP` taking them along. `CREATE DATABASE path
  [DESCRIPTION d]` makes the dictionary files byte for byte
  (`fixtures/sql/ddviews`, `ddprocs`, `dddatabase`); a `PASSWORD`, `ENCRYPT
  TRUE` and the AES types are refused with native 2200. `AdsDDAddView` now
  checks the statement's tables and columns as the original does, stores it
  as `CREATE VIEW` does, and refuses a derived table or a subquery with an
  `ORDER BY` and no `TOP` with 2115. A table is refused the name of a view
  or procedure (5132) as the original does; a procedure's clash with a table
  or view, and a table's or view's with a procedure, takes an object id and
  leaves a blank slot, as the original's does.
- **The options of the DDL that runs.** `CREATE [ONLINE] [UNIQUE] INDEX …
  CANDIDATE | NOTDELETED | IN FILE f | PAGESIZE n` (the index file of a
  dictionary's table, an index of a table of the dictionary joining it, and
  `DROP INDEX` leaving it), `CREATE TABLE … IGNORE TRANSACTIONS` and `ALTER
  TABLE … IGNORE | RESPECT TRANSACTIONS` (the table's transaction-free mark, a
  restructure that also sets it in the dictionary), `ALTER TABLE … DROP
  [CONSTRAINT] PRIMARY KEY`, `ALTER [COLUMN] c DROP NOT NULL | MINIMUM |
  MAXIMUM | ERROR MESSAGE | DEFAULT`, `NULL`, `NOT NULL` and `NOCPTRANS` in
  `CREATE TABLE` and `ALTER TABLE`, `ALTER ONLINE | ONLINE_WITH_TRANSITION
  TABLE` and `DROP TABLE … FROM DATABASE [NO_DELETE]` run as the original
  engine runs them (`fixtures/sql/ddopts.json`, `ddoptsdict.json` and the
  files they leave, byte for byte), with its refusals and the order it makes
  them in; `CONTENT` and the options of full-text indexes fail with native
  error 2200. An index file of any page size from 512 to 8192 reads, and a
  candidate order of a VFP table takes none of the blank record an `INSERT`
  that fails leaves.
- **`SET ::conn.Collation = …` and `SET ::stmt.Collation = …`** check their
  value as the original does (a name of the repository, `default`,
  `default_ansi`, `default_oem`, a locale after a colon), and keep none, as the
  original's statements see no effect of either
  (`fixtures/sql/setcollation.json`). A statement prepared again with the same
  text keeps the values of its parameters.
- **Scripts run as a statement.** A text of several statements, or one that
  starts with `DECLARE`, `IF`, `WHILE`, `RETURN`, `RAISE`, `EXECUTE IMMEDIATE`
  or an assignment, runs through `AdsExecuteSQLDirect` and
  `AdsPrepareSQL`/`AdsExecuteSQL` as the original runs it
  (`fixtures/sql/scripts.json`): its cursor is that of the last statement if
  that is a `SELECT` (any other statement or a loop's test after it lets go of
  it), `AdsGetRecordCount` counts the rows the statements wrote together, what
  an earlier statement wrote stays when a later one fails, parameters are shared
  across the statements, and an error is placed in the whole text (and given
  its line and column after the first line, which now goes for every
  statement). The script language's own checks are the original's too: a
  `DECLARE` after the first statement or of a name twice, a statement after
  `RETURN` or `RAISE`, an assignment to an undeclared variable (2218) or of a
  value of another type (2124).
- **`EXECUTE IMMEDIATE expr;`** in scripts runs the text as a script of its own,
  with none of the script's variables; its `SELECT`s are checked but not run,
  as in the original.
- **Temporary tables (`#name`)** work in SQL: `CREATE TABLE #t`, `SELECT`,
  `INSERT`, `UPDATE`, `DELETE`, `CREATE INDEX`, `ALTER TABLE` and `DROP TABLE`,
  a connection's own, named without regard to case, of the table type of the
  statement that made them, and gone when the connection closes
  (`fixtures/sql/temptables.json`). A name that is not there is 7112, an empty
  one 5019, one made twice 5189.
- **Cursors, `TRY … CATCH` and `CACHE` in scripts** fail with native 2200
  ("tflip does not support … yet") where they were syntax errors.
- **`SELECT … INTO table`** makes a free ADT table of the result (its columns
  as the plan types them, filled, a memo file for memo columns) and returns a
  live cursor on it, on a free and on a dictionary connection and under any
  table type of the statement; it is refused in a subquery, after a `UNION` and
  twice, as the original refuses it, and an existing table fails with 2010. The
  tables are byte for byte the original's (`fixtures/sql/selinto/`).
- **`EXECUTE PROCEDURE` in `FROM`**: `SELECT * FROM (EXECUTE PROCEDURE
  p(1, 2)) alias WHERE …` reads the result of a script or system procedure
  like a derived table, in joins and subqueries (`fixtures/sql/execproc.json`).
  `NCHAR` and `NVARCHAR` text joined with `+` takes the original's types.
- **SQL scalar functions that read the row, the statement or the connection**:
  `RECNO([alias])`, `DELETED([alias])`, `EMPTY(column)`, `ROWNUM()`, the `ROWID`
  pseudo-column (18 base 64 characters: the CRC-32 of the connection's path, of
  the table file's path or the table's object id in the dictionary, and the
  record number), `LASTAUTOINC(CONNECTION | STATEMENT)`, `LASTROWID()`,
  `LASTROWVERSION()`, `APPLICATIONID()`, `TTOC()` and `expr COLLATE
  ads_default_ci | ads_default_cs`, as probed in the original
  (`fixtures/sql/rowfuncs.json`, `rowid_paths.json`). `RECNO`, `DELETED` and
  `ROWID` work in the `WHERE` and `SET` of an `UPDATE` and a `DELETE`. `CHR`,
  `PAD`, `INLIST`, `TODAY` and `NOW_UTC` stay unknown functions (2158): the
  original's SQL does not have them either.
- **`MERGE`** runs: `MERGE [INTO] t [alias] [USING source] ON … WHEN MATCHED
  THEN UPDATE [WITH DELETE | WITH RECALL] SET … WHEN NOT MATCHED THEN INSERT
  [WITH DELETE] …` on ADT and DBF tables, with indexes, defaults,
  constraints, AutoInc columns and triggers, the files byte for byte
  (`fixtures/sql/merge`, `mergedd`). A source row sees what the rows before
  it wrote, `SHOW PLAN` shows it, and `INSERT … DEFAULT VALUES` and
  `DEFAULT` in a list of values work. `INSERT`, `UPDATE`, `DELETE` and
  `MERGE` of a `system.*` table fail with 5072, and an `UPDATE` whose text
  is cut to fit goes on with the other rows.

## v0.22

- **NChar and NVarChar orders take writes and weigh more characters.**
  Appending, changing, deleting, packing and reindexing a table with such
  orders (on one field or several, ascending, descending, unique,
  conditional) keeps them byte for byte as the original does
  (`fixtures/write/nchar_indexes`), and `CREATE INDEX` creates them
  (`fixtures/write/nchar_create`; the locale `en_US` is kept in the tag
  header). Their keys now cover every BMP character and plane 1 (probed,
  `tools/ace/seek/nkeys.py`), the contractions of UCA 5.1.0, ideographs,
  Hangul and unassigned characters by ICU's implicit weights, and Thai and
  Lao vowels before consonants; ~50 characters stay unsupported. The `…W`
  setters kept only the Windows-1252 part of text given to an NChar,
  NVarChar or NMemo field; and `AdsCancelUpdate` without an update in
  progress fails with 5085.
- **A seek through a cursor's temporary index** (`TIM_<n>`) works as the
  original's: `AdsSeek` (hard, soft and `SEEKGT`) and `AdsSeekLast` find the
  key among those of the records the cursor shows, for strings, numbers and
  dates, in live and static cursors, ascending and descending, and a key that
  cannot be converted leaves the cursor at EOF. The order a live cursor reads
  backwards is turned round in its own handle: `AdsIsIndexDescending` answers
  1 for `ORDER BY id DESC` on an ascending index (and 0 for `ORDER BY amount`
  on a descending one), `AdsGetIndexHandleByExpr` looks at the direction it
  reads, and moves through the handle start from its other end
  (`fixtures/sql/orderby.json`).

- **`AdsDDAddView` refuses a `SELECT` with an `ORDER BY` and no `TOP`** with
  2117 (`ORDER BY clause not legal in VIEW without TOP x`), the SQL engine's
  code as the return value and the 7200 text as the last error, as probed
  (`fixtures/sql/views.json`, which now records it and `CREATE VIEW`).

- **OEM tables convert the text clients give in filters and expressions**:
  `AdsSetFilter`, `AdsSetAOF`, `AdsEvalLogicalExpr`, `AdsEvalStringExpr`,
  `AdsIsExprValid`, `AdsLocate`, `AdsIsRecordInAOF` and index key and
  `FOR` expressions take their literals in the client's code page and
  compare and return them in the table's OEM one, as the original does
  (`tools/ace/oem`); `AdsGetFilter`, `AdsGetAOF` and `AdsGetIndexExpr` give
  the text back in ANSI.
- **The `AdsSet*` calls on a SQL cursor** answer as the original does
  (`tools/ace/errors/cursor_set.c`), not with `AE_NOT_IMPLEMENTED`. On a
  live cursor they change the table, as on the table itself; on a static
  one, or a live one of a read-only statement, they fail with 5072 once the
  value is converted for the column. A column that is not there fails with
  5113 by name and 5112 by number.
- **DBF tables in a dictionary, the corners**, probed and made to match.
  `CREATE TABLE` of a Clipper, FoxPro or Visual FoxPro table on a dictionary
  connection adds it to the dictionary (it made a free table), with its
  field rules and memo block size; a Visual FoxPro table takes `NULL`
  columns and a `PRIMARY KEY` (a candidate order of its `.cdx`, on a free
  connection too), the others refuse them as the original does (2215, 5042,
  5160). `AdsDDAddTable` keeps a DBF table's `ADS_OEM` (such a table reads as
  OEM through the dictionary, and a table created as OEM has no language
  driver byte), refuses `ADS_DEFAULT` as the type (5025, not
  `AE_NOT_IMPLEMENTED`), checks the character type (5025, 5027) and an index
  file's type (7015, 7017, 7108), and reports a file it does not find as 7041.
  A candidate order refuses a NULL key with 5208 (it failed with
  `AE_NOT_IMPLEMENTED`), and a refused `INSERT` leaves a blank deleted
  record. `system.tables` reports a DBF table's type, and `DROP TABLE` of a
  dictionary's DBF table takes its memo file too.
- **A cursor's `ORDER BY` has a temporary index**, as in the original
  engine: a live cursor whose `ORDER BY` no order serves, and a static cursor
  sorted on its columns, show a `TIM_<n>` index in `AdsGetNumIndexes`,
  `AdsGetAllIndexes` and `AdsGetIndexHandleByOrder`, with the key expression
  the original writes (`name;Descend(id)`, `UPPER(name)`, `(0-id)`), its
  flags and key counts; `AdsGetIndexHandle` with no name gives the order a
  cursor reads, a table's or the temporary one. The `ORDER BY` of an
  expression the original cannot write as an index is a static cursor's, as
  there; a temporary index turns round only if every key is descending, so
  `ORDER BY a, b DESC` puts NULL text first. Probed with
  `tools/ace/sql/orderby.txt` (`fixtures/sql/orderby.json`).
- **Views of one table's columns are live cursors**, as in the original
  engine: `SELECT * FROM view` opens the table itself when the view's
  `SELECT` reads one table's columns, some of them or renamed, with a
  `WHERE` or without, or reads another such view; a `SELECT` of the view
  with a `WHERE` or an `ORDER BY` does too, and its plan is the table's.
  Views of anything else stay static, as in the original. Probed with
  `tools/ace/sql/views.txt` (`fixtures/sql/views.json`), whose `sql.c` gained
  the index functions, moves through a cursor and through one of its orders,
  and edits. `AdsGetIndexExpr` of a cursor that shows only some of its
  table's columns no longer leaves the fields it does not show as `F<n>`.
- **Binary orders of more than 62 leaves** are built and kept current: a
  bitmap that outgrows the 62 leaves its root node holds (over 246,000
  records, with nodes of 512 bytes) gets the original engine's second
  interior level, byte for byte (`fixtures/write/bitmap_split`,
  `bitmap_levels` and the CDX twins): the root's leaves move under an
  interior node when the 63rd starts, interior nodes are added as they fill,
  and a reindex builds the same. Bits are set and cleared through the second
  level. The third level (over 15 million records) is still refused. Found
  on the way: a Clipper, FoxPro or VFP table whose records take under 5
  bytes could not be written, and building a binary ADI order took time
  squared in the table's records.
- **Property setters continue long properties in `More Property`.** A view's
  statement, a procedure's script or a user-defined property that grew past
  the dictionary record's property field made `AdsDDSetViewProperty` and its
  siblings fail; they are stored in the memo file now as the original engine
  stores them (`tools/ace/dd/proplong.c`, `fixtures/dictionary/proplong`):
  moved, shrunk back into the record, and the object's comment replaced,
  byte for byte.
- **`AdsGetRecordCount` on a connection is an error.** It answered 0 with
  success; the original gives `AE_INVALID_HANDLE` (5018, "Must be statement,
  table, cursor or index order handle."), after checking the option and the
  count pointer (`Parameter 2` and `Parameter 3` in their error texts).
  `tools/ace/errors/record_count.c` records it.
- **`AdsSetCollation` and `AdsStmtSetTableCollation` refuse an unknown
  collation name.** A name `adscollate.adt` lacks fails with 7147 (the name
  after the text), as in the original; one it has is kept in its stored
  spelling, regardless of case and trailing blanks. The statement call also
  takes `ANSI`, `OEM` and `:` with an ICU locale (`:en_US`, `:de_ADS_CI`),
  refusing a locale without a collator with 1564. Probed on a free-table, a
  local dictionary and a remote dictionary connection, which take exactly the
  same names (`tools/ace/collation/setcollation_dd.c`). Without the collation
  files beside the DLL a name is kept as before.
- **A finished transaction is no longer recovered by another process.** Its
  log was closed before it was removed, so a process recovering meanwhile
  could take it for a crashed one and hold its tables exclusively for a
  moment: the next open of one failed with 7008. The log is emptied first
  now. Seen on Windows, where the workspace's tests run now as well.
- **Mapped drives reach the share's server.** On Windows a connection's path
  is named as the original names it (`tools/ace/mapped`): full, and through a
  drive mapped to a share by the share's UNC path, without a trailing
  separator. So `X:\data` with a remote server type connects to `tflipd` on
  the share's host, `AdsGetServerName` reports that host and
  `AdsGetConnectionPath` the UNC path, and a table or connection named
  through the drive is found by it.
- **The `sql` log channel is off by default**, as `err` is: it logs every
  statement, so a busy client floods stderr. `TFLIP_DEBUG=+sql` turns it on.
- **`ADS_DATABASE_TABLE` without a dictionary is refused** with 5025, as in
  the original engine, before the file is looked for: `AdsOpenTable` handed
  out a handle to a table it had not read, even of a file that was not
  there. Its other refused types, and those of `AdsStmtSetTableType`, name
  the parameter in the last error's text.
- **The `AdsSet*` calls on a connection's handle** fail as in the original
  (5018, or 5023 for `AdsSetBinary` and `AdsFileToBinary`, each with its
  detail), not with `AE_NOT_IMPLEMENTED`. `AdsOpenTable90` and
  `AdsClearCallbackFunction` no longer report themselves as stubs on the
  `fixme` channel.

## v0.21

- **SQL compares text by the statement's dynamic collation**
  (`AdsStmtSetTableCollation`, else the connection's `AdsSetCollation`):
  `WHERE`, joins, `ORDER BY`, `DISTINCT`, `GROUP BY`, `UNION`, `MIN`/`MAX`
  and `LIKE`, literals' text too, and `UPPER`/`LOWER` take its case. As in
  the original engine it weighs the text four ways: comparisons and
  `MIN`/`MAX` as the keys and then by which characters expand (`ß` after
  `ss`), `GROUP BY`, `DISTINCT` and `UNION` taking those as equal with a
  blank first, a temporary index's `ORDER BY` by the keys, and a live
  cursor's `WHERE` as `GROUP BY` (see `docs/ADS_BUGS.md`); of equal values
  `MIN`, `MAX` and `UNION` keep the first. A table whose orders have another
  dynamic collation than the statement's is refused (7200, native 5209), as
  in the original. Probed with `tools/ace/sql/collation_dynamic.txt`
  (`fixtures/sql/collation_dynamic.json`); the probe's `sql.c` gained
  `setcollation`.

- **`CREATE TABLE` and `DROP TABLE` on a dictionary connection** add the
  table to the dictionary (as `AdsDDAddTable` would) and take it out with
  its files, the dictionary and the files byte for byte as the original
  engine writes them: `CONSTRAINT NOT NULL`, `DEFAULT 'value'`,
  `CONSTRAINT MINIMUM`/`MAXIMUM`/`ERROR MESSAGE`, `MEMOBLOCKSIZE` (kept in
  the dictionary), `IN DATABASE`, `AS FREE TABLE`, and the `PRIMARY KEY` of
  a column or of the table (`PRIMARY KEY (a, b DESC)`, `CONSTRAINT name
  PRIMARY KEY`), which the table records as its primary key. The new
  objects take the slots of deleted ones first, and the dictionary lists
  its objects in record order, as the original does. The refusals match:
  two primary keys (2117), a rule given twice (2215), an unknown column
  option (2117), a key column that is a memo (2205) or missing (2121), `IN
  DATABASE` on a free connection (2215), rules on an `AS FREE TABLE`
  (5132), `DROP TABLE` of a dictionary's table on a free connection
  (5159). On a free connection a plain `NOT NULL` and a lone `CONSTRAINT
  MINIMUM` now pass and the 5160 names the first column with a rule, as in
  the original; a table-level `PRIMARY KEY (…)` works there too, and a key
  of several fields whose last is text pads with blanks. A table bound to
  a dictionary now carries 2 at header offset `0x14` whatever its object
  id, as the original stamps it (it was the id, right only for id 2).
  Probed with `tools/ace/sql/ddtables.txt` (`sql.c` gained `ddcreate`):
  `fixtures/sql/ddtables.json` and `fixtures/sql/ddtables/`.

- **A timestamp seek key and `CTOTS` read their text as the engine does**,
  as a TimeStamp value's text is read: the date ends at the text's third
  separator in any date format, and the time after it is read as the
  setters read it (any separator, `13:00 AM` 01:00), but not checked (`CTOTS`
  keeps 25 hours; a seek key seeks past the day's end); `CTOTS` keeps the
  time when the date is wrong or missing. Dates in formats without
  separators (`CCYYMMDD`, `DDMMYY`) are read as the engine reads them too:
  spaces left out, the year up to four digits even as `YY` (`081117 12:34`
  in `DDMMYY` is 1712-11-08). Verified on every text of the new
  `fixtures/fields/tstexts.tsv` (CTOTS, AdsSetField and AdsSeek on a
  TimeStamp order) and of `fixtures/fields/dates.tsv`, which has dates in
  formats without separators now.

- **A dictionary's rules apply in the original's order against referential
  integrity, and its defaults are expressions.** A record's constraints come
  first, then its BEFORE triggers, then its referential integrity, through
  the table API as through SQL; `AdsWriteRecord` now also refuses a parent's
  key change a RESTRICT rule's children hold (5141) and re-points CASCADE,
  SET NULL and SET DEFAULT children, which it left alone. Children are
  re-pointed as through the table API, so a child that refuses is written
  unchanged (its triggers firing) and the refusal (`5147`–`5149`) is
  reported; the parent then stays as it was, without transactions too. A
  table-API delete that a SET NULL or SET DEFAULT child refuses returns
  success and leaves the parent, as a RESTRICT one does. A rule's messages
  end its errors (7076, 5141). A default, minimum or maximum is read as an
  expression on the record being appended (`2+3`, `'a'+'b'`, `STOD(…)`,
  another field), taken if the field holds its type, else as text; `NOW()`
  is no longer taken for a Date or Time field. `AdsDDSetFieldProperty`
  refuses a default for a memo, binary or automatic field, a minimum or
  maximum for those and logical and binary-string fields (5066), and a
  default, minimum or maximum the field cannot take, with the setter's error.
  Probed with `tools/ace/dd/rules.ops` (`fixtures/dictionary/rules.json`, on
  the new `dictionary_rules` fixture). An empty default the dictionary keeps
  gives an inserted field its empty value (an Integer's 0) rather than NULL,
  and a record the rules refuse to an `INSERT` keeps its values in its
  deleted slot, as in `fixtures/sql/ddtables` (whose `cons.adt` is now
  compared too). An error variant now carries a
  message: protocol version 75.
- **A statement holds the tables its `SELECT` read until it closes**, not
  only until its cursor closes, as the original engine does (probed,
  `tools/ace/cache`): on `tflipd`, and on the local server with `[cache]
  local = yes`. Meanwhile the tables count as open (`AdsMgGetOpenTables`),
  another connection cannot open them exclusively (7008) and `DROP TABLE`
  fails (7200, native 7008); `AdsCloseCachedTables` leaves them; running the
  statement again lets go of its last run's tables; closing it hands them to
  the statements' cache (`AdsCacheOpenCursors`). `DROP TABLE` of a table any
  handle of the process has open now fails the same way instead of deleting
  it. The cache probe's goldens (`fixtures/cache/`) gained a section for it.
- **More AOF terms are looked up in the orders, with the original's levels
  and records**: `EMPTY()` and `ISNULL()` of a key (also `= .T.`/`= .F.` of
  them, now fully optimized as in the original), `$` on a text key, terms on
  the leading part of a FoxPro key of several fields (`a` and `a+c` of the
  key `A+C` that `CREATE INDEX … (a, c)` makes; `YEAR(d)` through
  `DTOS(D)+A`), which were not optimized at all before, and text under a
  static collation (`ANSI_CHAR_SET`) in ADI and CDX orders. Where the orders
  find other records than evaluating the term would, the AOF shows the
  orders' as the original does: `$` compares the key's bytes (a VarChar key
  padded, a CICharacter key in lower case, the value as it is), `ISNULL()`
  through a FoxPro order finds the blank keys, and `a = 'ab   x'` through
  `A+C` finds the keys that start with it (see `docs/ADS_BUGS.md`). In an
  ADT table, `.NOT.` of `ISNULL()`/`EMPTY()` is fully optimized only alone,
  as in the original, and an `.OR.` with a part found partly that leaves
  nothing to evaluate leaves nothing. Probed in `fixtures/write/aofs_more`
  and `fixtures/write/aofs_static`.
- **The last AOF terms the orders serve are looked up too**: `EMPTY()` and
  `ISNULL()` of Visual FoxPro keys (blank keys and NULL keys), `EMPTY(k) =
  .F.` (the keys neither empty nor NULL: through a Visual FoxPro order NULL
  stays out, as in the original, though evaluating `EMPTY()` of NULL there
  gives `.F.`; `ISNULL(k) = .F.` through a FoxPro order leaves the blank
  keys out), and under a static collation numbers and dates of CDX orders
  and CICharacter text of ADI orders. `$` through an order of a dynamic
  collation is not optimized at all, as in the original (it was reported as
  fully optimized). Probed in `fixtures/write/aofs_rest` and
  `fixtures/write/aofs_static_keys`.
- **Static collations**: CDX keys of numbers and dates are ordered by their
  bytes, not weighed by the collation, so seeks and new keys find their place
  among them as in the original; CICharacter keys of ADI orders are folded by
  the collation's lower case (`Äpfel` as `äpfel`), and `==` and `$` on
  CICharacter text fold case by it too. `EMPTY()` of NULL on a Visual
  FoxPro table is `.F.`, as in the original.

- **`EMPTY()` on a DBF table holds for blank values** (blank text, zero,
  `.F.`, the empty date), as in the original; it held for NULL alone, as on
  an ADT table. And under a static collation, a comparison with `UPPER()` or
  `LOWER()` of text (which give the collation's weights) no longer weighs
  them twice: `UPPER(name) = 'ÄPFEL'` under German finds `Äpfel` and
  `äpfel`.
- **A dictionary's encrypted table opened as a free table is refused as the
  original engine refuses it**: a table of a dictionary that encrypts its
  structure (RC4) is `7060` ("Invalid encryption password", naming the
  file), whatever password the connection has, and `5159` ("The table is a
  database table. It cannot be opened as a free table.") while its dictionary
  has it open; through SQL, the AQE error 7200 with that native error. It
  was `AE_TABLE_ENCRYPTED` (5098). Probed with `tools/ace/crypto/freeopen.c`
  (`fixtures/write/encryption/freeopen.json`). A new error variant extends
  the wire format: protocol version 74.
- **CDX orders on an expression whose value is a timestamp are built and
  kept current** (`CTOTS(c)`, `STOTS(…)`, `CREATETIMESTAMP(…)`), on Visual
  FoxPro and FoxPro tables: a table with one is written to (it was
  `AE_NOT_IMPLEMENTED`), `AdsCreateIndex61` makes one, and timestamp text
  or a double seeks it. The key is a VFP TimeStamp field's, but with the
  milliseconds cut off rather than rounded, as in the original. On a VFP
  table an order on any expression that reads a field taking NULL
  (`UPPER(c)`, `STR(i)`) now gets keys led by the NULL byte, as the
  original makes it (tflip gave it that byte only for a field alone), and
  seeks find them. A FoxPro order of timestamps built on records is read and
  kept current as the original does it, trailing zeros taken for spaces, so
  that the original's keys ending in zeros are not found (see
  `docs/ADS_BUGS.md`). And when the last interior node of a level splits
  because the leaf below it halved, it halves too (it kept all but the new
  entry, as it does when the leaf below took a key at the order's end).
  Probed with `tools/ace/write/cdx_ts.py` (`fixtures/write/cdx_timestamps`,
  byte for byte).

## v0.20

- **The local server can keep closed tables open for reuse**, as the
  original's local server does: with `tflip.ini`'s `[cache] local = yes` (or
  `TFLIP_LOCAL_CACHE=yes`), its connections keep the tables of 25 closed
  statements open by default and as many closed tables as
  `AdsCacheOpenTables` says, the same way `tflipd` keeps them, and lowering
  `AdsCacheOpenCursors` closes the statements' tables at once, as there; the
  probe's local output (`fixtures/cache/local.txt`) now replays. It is off by
  default: a kept table holds its file open in the client's process, so
  another program could not open it exclusively until the client closes its
  cached tables or its connection.

- **A table-API write's or delete's triggers run in a transaction of the
  record's own**, as an SQL statement's do: with real transactions, what a
  failing trigger wrote is taken back with the record's change (a refused
  `AdsDeleteRecord` still returns success), as the original's remote server
  does. The constraints golden (`fixtures/dictionary/constraints.json`) is
  now recorded on the remote server (`constraints.sh … remote <share>`); the
  local server's, which keeps those writes, is
  `fixtures/dictionary/constraints_local.json`, replayed with
  `[transactions] mode = count`.

- **An encrypted table's password is the handle's, as in the original
  engine** — before, it was the open file's, which every handle on it shares
  in the process: once one handle had given the password (opening the table
  with it, `AdsEnableEncryption`, or through its dictionary), another handle
  opened without it — of the same connection, of another, or of another
  client of `tflipd` — read the records and memos deciphered and could write
  them, and a table of a dictionary that encrypts its structure opened as a
  free table without the password. Now a handle that has not given the
  password reads an RC4 record enciphered, an AES one blank and the memos of
  either empty, is refused changing an encrypted record (`AE_RECORD_ENCRYPTED`),
  appending to or writing an encrypted table and (de)ciphering records or the
  table (`AE_ENCRYPTION_NOT_ENABLED`), writes the clear records of a table
  only some of whose records are encrypted in the clear, and does not open
  such a dictionary's table; `AdsDisableEncryption` lets go of one handle's
  password only; and `AdsEnableEncryption` with a wrong password on a table
  that has one is refused (7060) where it set a new password check over the
  table's. `AdsIsRecordEncrypted` is false for an AES record, as in the
  original. Verified against `fixtures/write/encryption/share.json`
  (`tools/ace/crypto/share.c`, the original engine under Wine), locally and
  over `tflipd`. Protocol version 73: three engine errors.

- **`AdsGetLastError` adds the original engine's details after any
  failure**, not only a setter's: the file an open did not find or could not
  open (`<data>\gone.adt`), the order a unique key violates
  (`The key value supplied for u:BY_CODE is not unique.`), the field of a
  binary memo a getter refuses, `Top` or `Bottom` of a scope not set,
  `Parameter 3` of an option refused, why a restructure stops (the deleted
  field an order uses, the field and record a value failed on) and why a
  field definition or index option is refused, and the AOF calls'
  `You must call AdsSetAOF first.`. The write fixtures now compare every
  command's text (they compared only the setters'); three SQL texts the
  engine words differently are matched along the way, and a restructure
  names the order whose key uses a deleted field before one whose condition
  does (`fixtures/write/error_details` for the rest).

- **The last error after a wrong handle is the original's**, whatever ran
  before: a table call's check keeps an error left before, `AdsGotoTop`'s,
  `AdsCloseTable`'s and `AdsCloseSQLStatement`'s replace it, a value that is
  no handle leaves `AE_INVALID_HANDLE` though the call returns its own code,
  and each check's text has its detail (`The handle must be to a table.`).
  `AdsWriteRecord` refuses an index order's handle, as the original does
  (`fixtures/errors/handle_order.json`). Protocol version 72: an engine
  error carries its detail to the client.

- **A TimeStamp value's text is read as the engine reads it.** Its date part
  ends at the separator after the date's (a run of characters that are not
  digits, after a digit) and is read as a Date value is; the time follows
  (`1108/2017 13:00` is midnight, `11/08/201712:00` too, `1182017 01:02`
  0701-11-20). Verified on every text of `fixtures/fields/dates.tsv`, with
  new ones on where the time starts.

- **SQL takes a Clipper or FoxPro table's empty values as the engine does.**
  Those tables have no NULL: an empty field (blank, 0, false, spaces) is NULL to
  `IS NULL`, `IFNULL` and `COALESCE`, and its value to comparisons, arithmetic
  and aggregates — `COUNT` counts a blank one, `AVG` divides by it, `MIN` of a
  blank number is 0 and of a blank date NULL. A blank field shows as NULL in a
  result (but text, doubles and integers) and is a group, and a `DISTINCT`
  value, of its own. A VFP table's blank date is 1899-12-30 in SQL, and a DBF
  numeric column of a static cursor is the ADT numeric of its precision
  (`fixtures/sql/dbf.json`).

- **`TOP n` works out the select list only for the rows it keeps**, after
  every row's sort keys (a key that is a column of the select list works that
  column out for every row, once), as the original engine does; a
  `TOP … PERCENT` or `TOP … DISTINCT` still works it out for every row. It
  shows with `RAND()`, whose sequence now goes on as the engine's does after
  such a `SELECT` (`fixtures/sql/funcs.json`).

- **`GROUP_CONCAT` of a `CICHAR` column is the engine's case-insensitive
  `CICHAR(1024)`**, padded to its width, its inner `ORDER BY` of such text
  regardless of case (equal keys in record order, reversed when the last key
  descends). Found along the way and matched: the text functions of a `CICHAR`
  argument (`TRIM`, `LTRIM`, `LEFT`, `SUBSTRING`, `REPLACE`, `CONCAT`,
  `SOUNDEX`, ...) are case-insensitive `CICHAR`s of their width too, so they
  compare regardless of case, and the `TRIM(… FROM …)` form in a `WHERE` makes
  a static cursor (`fixtures/sql/groupconcat.json`).

- **DBF tables in a dictionary.** `AdsDDAddTable` adds Clipper (NTX), FoxPro
  (CDX) and Visual FoxPro tables, no longer `AE_NOT_IMPLEMENTED`: the table's
  type kept in its Table object, a VFP field's binary and NULL flags in its
  Field object's options, NTX index files (their one order named after the
  file, not compound) and CDX ones as the original engine writes them, and the
  table's files left as they were (only an ADT table is bound). A dictionary's
  DBF table opens by name, and in SQL, as its own type with its index files,
  `ADS_DD_TABLE_TYPE` reports it, and a table opened by name reports
  `ADS_DICTIONARY_BOUND_TABLE` among its open options. Found along the way: a
  DBF Numeric column in a static cursor is one longer, as the ADT Numeric keeps
  a place for the sign. Byte for byte against a dictionary the 12.0 engine made
  under Wine (`fixtures/dictionary/dbftables`, `tools/ace/dd/ddwrite.c`), and
  replayed against what it does with it (`fixtures/sql/dbfdict.json`,
  `tools/ace/sql/dbfdict.txt`).

- **Long view statements.** `AdsDDAddView` with a `SELECT` too long for the
  dictionary record's property field (more than 268 bytes) no longer fails as
  not implemented: as in the original engine, the properties go on into the
  record's `More Property` memo, placed in the memo file as the engine places
  them (a continuation larger than 32 blocks counting twice among its
  allocations), before the view's comment. Byte for byte against dictionaries
  the 12.0 engine made under Wine (`fixtures/dictionary/viewlong`,
  `tools/ace/dd/ddwrite.c`).

- **A dictionary's links outside SQL.** A listing of
  `ADS_DD_TABLE_VIEW_OR_LINK_OBJECT` and `AdsFindFirstTable62`/
  `AdsFindNextTable62` without a mask now list the tables (not the views) of each
  global link the user may use, where the link stands among the dictionary's
  objects — `:link::table` as an object, the table with the link's name as a
  table — logged into as the link's user, whose rights decide what is listed; a
  link that does not log in is left out. `AdsGetNumActiveLinks` and
  `AdsGetActiveLinkInfo` (`name;user;dictionary`) report the connection's active
  links, newest first: a link made on it that logs in, and one a statement used
  (a dictionary's path as a link without a name); dropping a link, and a listing
  that followed it, make it inactive, as in the original engine. Listings of
  tables, views and links on a dictionary that verifies rights show a user only
  what the user holds a right on (links: `ACCESS`), and the end of a dictionary's
  table listing leaves 5137 as the last error. `AdsDDCreateLink` refuses a link
  of the connection's own with a wrong password (5097) and a global link to a
  user who is not the administrator or in `DB:Admin` (5054); `AdsDDDropLink`
  drops a link of the connection's own. A table is not opened through a link by
  name (`AdsOpenTable`: 5132), as in the original. Verified against the live
  12.0 engine under Wine (`fixtures/sql/linklist.json`,
  `tools/ace/sql/linklist.txt`), and the links of `fixtures/dictionary/dd.json`
  now replay. Fixed along the way: a `tflipd` share configured with `..` in its
  folder did not hold the files under it for a link.

- **SQL `GRANT` and `REVOKE`, object type 0 and the right to create.** `GRANT
  permission, … [ON object] TO grantee [WITH GRANT]` and `REVOKE [GRANT OPTION
  FOR] permission, … [ON object] FROM grantee` now run on a dictionary: `SELECT`,
  `INSERT` and `UPDATE` (on the object or one of its columns), `DELETE`,
  `EXECUTE`, `ACCESS`, `ALTER`, `DROP`, `INHERIT`, `ALL`, `ALTER DATABASE`,
  `INHERIT DATABASE` and `[INHERIT] CREATE TABLE | VIEW | USER [GROUP] |
  PROCEDURE | LINK | PUBLICATION | SUBSCRIPTION | FUNCTION | PACKAGE`. As in the
  original, each permission is one grant in turn and the first that fails ends
  the statement, keeping those before it; the object is found by its bare name
  among every kind; the syntax errors (2115, 2117, 2103) and the engine's own
  refusals are the original's, word for word. `AdsDDGrantPermission`,
  `AdsDDRevokePermission` and `AdsDDGetPermissions` take object type 0
  (`ADS_DD_UNKNOWN_OBJECT`): the name is looked up among every kind (the
  original keeps names unique across them, so no kind has to win), 5137 if it
  is none. `ADS_PERMISSION_CREATE` with no object name — alone, with the grant
  option, or with `ADS_PERMISSION_INHERIT` for the user's inherit bit — grants
  and revokes the right to create a kind of object, kept in the grantee's own
  object (a byte per kind), and `system.permissions` /
  `system.effectivepermissions` now fill their create rows from it.

  Probing them corrected rules every grant shares, now matched: on a table, the
  rights of each column are kept only while the columns differ, a table-wide
  grant or revoke reaches every column, and a column's insert right is its own
  bit (`0x04`); every call raises the dictionary's version, even one that
  fails; a grant of the inherit bit alone where the user has none recorded
  writes nothing, and the inherit bit cannot carry the grant option; a column
  belongs to a table, not a view; `system.permissions` reports a right held
  with the grant option as 2; and the error texts of each refusal (unknown
  object or grantee by kind, rights a kind does not allow, built-in groups).
  Verified by replaying the 356 steps recorded from the live 12.0 engine
  (`tools/ace/dd/grants.c` running `grants.txt`, `fixtures/dictionary/grants.json`)
  — return codes, error texts, masks and six `system.permissions` dumps — and
  against ten golden dictionaries byte for byte, and both ways: the same probe
  run through tflip's own `ace64.dll` gives the engine's results (but for
  `DB:Public`, below). The earlier `pubperm` replay now runs its SQL and type-0
  calls too. Through the DLL, a null object name is refused as the original
  refuses it (5132 before the dictionary is touched), and `AdsDDGetPermissions`
  with one finds nothing (5132, not an invalid pointer). Rights granted to `DB:Public` are kept
  in the object itself under a checksum not worked out, so granting them is not
  implemented yet (see KNOWN_ISSUES); the engine's refusals for it are matched.

- **Skipping from a record an order has no key for** (one its `FOR`
  condition leaves out, or a deleted ADT record gone to with
  `AdsGotoRecord`) goes where the original engine goes: in a CDX order a
  skip of n lands on the nth key from where the record's entry would be (by
  key, then record number); in an ADI order the first step goes to the key
  beside that place and each further one two keys, so a skip of n lands on
  the (2n − 1)th, as in the original; in an NTX order any skip goes to EOF
  or BOF. tflip used to go to EOF or BOF in every order, and from a deleted
  ADT record counted one key a step (`fixtures/write/skip_unkeyed`).
- **Visual FoxPro TimeStamp orders are built and kept current.** A
  TimeStamp's CDX key is the double the original makes of it: the time
  rounded to the second, `(day × 86400 + seconds) × (1/86400)` — one bit
  below the nearest double for about a quarter of the values — and, for a
  time that rounds up to midnight, the original's odd key just below the
  next day (see `docs/ADS_BUGS.md`). So a table with such an order is
  written to (it was `AE_NOT_IMPLEMENTED`), `AdsCreateIndex61` makes one,
  `AdsExtractKey` builds the key, and seeking timestamp text finds those
  keys, the text's milliseconds left out as in the original
  (`fixtures/write/vfp_timestamps`, 446 values, byte for byte).

- **A dictionary's own rules apply on write.** Field defaults fill a record as
  it is appended (`AdsAppendRecord`, an SQL `INSERT`), so they read back before
  it is written; a Date, Time or TimeStamp default of `NOW()` or `TODAY()` is
  the time of the append, any other the text as `AdsSetField` sets it. A
  field's constraints are checked as it is set — NULL in a field that cannot be
  NULL (5147; `AdsSetEmpty` and an empty memo are NULL), below its minimum
  (5148) or above its maximum (5149) — and a refused value is not set; through
  SQL, in the order the columns are listed. Writing the record checks each
  field again in field order (a default may break a constraint, as may a row
  written before the constraint was set), then the table's record validation
  expression (5150, a NULL result failing). The texts are the dictionary's
  field or table message where it has one, else the engine's own naming the
  column; SQL reports them as 7200 with the code as its native error. Triggers
  now fire on the table API too (`AdsWriteRecord`, `AdsDeleteRecord`), after
  the constraints and before a BEFORE trigger: an INSTEAD OF trigger replaces
  the write, and a failed trigger is 5177 (`Trigger execution failed.` with the
  trigger's error) through both the table API and SQL — except a failed BEFORE
  DELETE through the table API, which leaves the record and returns success,
  as the original does. In SQL the BEFORE triggers now fire after the
  record's checks, and `__new` carries the defaults. Scripts take `RAISE
  name(code, message)` (2224, located at the name), and a trigger's `IF` and
  assignments read `__new.col`/`__old.col`. Setting a field of a deleted
  record (where a failed `AdsGotoRecord` leaves the table) is refused with 5035,
  as the original refuses it, rather than writing over the free list. All
  checked against the original engine (`fixtures/dictionary/constraints.json`,
  from the new `tools/ace/dd/constraints.c` on the new `dictionary_constraints`
  fixture). The wire protocol is at version 70.

- **Clients trust `tflipd` certificates from a CA.** Besides a certificate
  whose fingerprint is configured, a client now trusts one that chains to a
  root the operating system trusts — its certificate store on Windows, the
  usual trust store elsewhere (`rustls-platform-verifier`, a new dependency) —
  and names the server as the client named it, so a `tflipd` given a CA's
  certificate (`--cert`, `--key`; the file may carry the intermediate
  certificates, which `tflipd` now presents) needs no fingerprint on the
  clients. `tflip.ini`'s `[trust]` section can add the roots of a private CA
  (`roots`, a PEM file) or turn the system's off (`system = no`, fingerprints
  only); `TFLIP_TRUST_ROOTS` and `TFLIP_TRUST_SYSTEM` override it. A
  fingerprint is still trusted whatever the certificate names or who issued
  it.

- **Connections to `tflipd` can be compressed.** The client's setting comes
  from the `Compression` connect-string option (`INTERNET`, `ALWAYS`, `NEVER`)
  or `AdsConnect60`'s `ADS_COMPRESS_*` options, else from `tflip.ini`'s
  `[connection] compression` or `TFLIP_COMPRESSION` (where the original reads
  `ads.ini`); the server's from `tflipd --compression` (`internet` by
  default). Client and server agree on it in their greeting as the original's
  do: never if either says never, else always if either says always, else
  only for a connection as to an Internet server (`ADS_AIS_SERVER`). A
  compressed connection is one deflate stream each way, under TLS, so a
  message compresses against those before it. As in the original engine, a
  bad `Compression` value fails with 5019 (the value added to its text) before
  any server is tried, also on the local server; `AdsMgGetConfigInfo` reports
  the server's setting as an `ADS_COMPRESS_*` number for `tflipd` (0 on the
  local server); and `AdsConnect60` now accepts `ADS_REPLICATION_CONNECTION`.
  Probed against the 12.0 engine and evaluation server under Wine
  (`tools/ace/compress`). The wire protocol is now version 71.

- **Triggers: `CREATE TRIGGER`, `DROP TRIGGER`, CONFLICT triggers, `NO MEMOS`
  and `NO TRANSACTION`.** SQL now makes and drops a dictionary's triggers
  (script bodies, kept as written, checked as they are made; a long one goes
  on in `More Property`), refusing as the engine does a `CONFLICT INSERT`
  trigger (2215), a second `CONFLICT` trigger of an event and a `BEFORE`
  beside an `INSTEAD OF` (5181), and a trigger not named with its table
  (5132). CONFLICT triggers are kept but, as in the original, never fire on a
  table's own changes (only replication fires them). A `NO MEMOS` trigger
  reads its memo and BLOB columns as NULL in `__new` and `__old`. With real
  transactions each record's change runs with its triggers in a transaction
  of its own, two levels deep: a failing trigger takes back that record's
  change and the triggers' writes, leaving earlier records and the client's
  transaction be; `ROLLBACK` in such a trigger is 5047; `NO TRANSACTION`
  triggers keep their writes. The record a failing `AFTER INSERT` fired on
  goes either way. Along the way a failing script statement is located in
  the script (`** Script error information`), and a trigger reads
  `::conn.TransactionCount`.
  Verified against the 12.0 remote server under Wine
  (`fixtures/sql/triggers.json`, `tools/ace/sql/triggers.txt`; `sql.c` can
  now connect to a remote server and has `txcount` and `cachecursors`).
  Found along the way: the original's cursor cache keeps a table's old
  triggers (`docs/ADS_BUGS.md`).

- **With `ANSI_CHAR_SET` empty, the static collation follows the host's
  locale**, as in the original engine. Where the user locale's language
  abbreviation is `ENU`, `ENG` or `ENC` (US, UK or Canadian English) it stays
  the default one that weighs every byte as itself; under any other locale
  (Australian English included) each byte weighs its rank in Windows' own
  sorting of the 256 one-byte strings under that locale, equal ones by byte
  (`ä` just after `a` under German, after `z` under Swedish), with the
  locale's upper and lower case — not `ansi.chr`'s collation of that
  language, and needing no collation files at all. The system locale plays no
  part, and the OEM collation stays the default with `OEM_CHAR_SET` empty. On
  Windows tflip asks Windows, as the engine does; on Linux (`tflipd`, or the
  library elsewhere) the locale is `LC_ALL`, else `LC_COLLATE`, else `LANG`
  (`de_DE.UTF-8` is Windows' `de-DE`; `C` and none are `en-US`), and the
  collation comes from tables tflip carries for every locale Windows knows,
  derived under Wine. Index keys, orders, the collation an ADI records (so
  which indexes open, 5175), `UPPER`/`LOWER` and comparisons all follow.
  Verified against the live 12.0 engine under Wine for 24 locales (Latin,
  Cyrillic, Greek, Hebrew, Arabic, Thai, Vietnamese, the four double-byte
  ones) and on Windows 10 with the user and system locales set apart
  (`fixtures/collation/sysdefault.json`, `tools/ace/collation/sysdefault.c`);
  tflip's own `ace32.dll` under Wine gives the same.

## v0.19

- **SQL reaches the tables of a dictionary's links.** `link.table` in a
  `SELECT` (joins, unions, subqueries, aliases, live and static cursors), in
  `INSERT`, `UPDATE` and `DELETE`, and in a view — `sp_GetColumns` describes a
  view reading one, carrying the linked table's defaults through. A name that
  is no link is a dictionary's path relative to the connection's dictionary
  (`"..\other\other.add".table`). As in the original engine, the linked
  dictionary is logged into as the link's user (or the connection's own, for
  `ADS_LINK_AUTH_ACTIVE_USER` and for a path), its tables alone are reached (not
  its views), its access rights, referential integrity and triggers hold, and
  its writes stay out of the connection's transaction; a failed login is 5174
  naming the link and why (7041, 5100, 5097), a user of a dictionary that
  verifies rights needs the link's `ACCESS` right (5054), an unknown table is
  5132, and on a free connection any qualified name is 5041. `DROP TABLE`/`ALTER
  TABLE` of a link's table are 2125, `CREATE TABLE`/`CREATE INDEX` 2115, and a
  name of three parts 2117. `AdsDDCreateLink` now checks the linked dictionary
  as the engine does (7041 for a missing file, 5100 for an unknown user, 5054
  for an anonymous link to a dictionary that requires logins; the password is
  not checked), refuses an alias in use with its own code (5132, not 7200),
  keeps a link that is not global on the connection alone (unwritten, as the
  engine does), and stores a static link's full path.

  Found along the way and matched: a live cursor on a dictionary's table
  reports `ADS_DICTIONARY_BOUND_TABLE` and only the permissions its user holds
  among its open options; 7079 names the table; a `DELETE` the user may not do
  is 7087; a referential-integrity refusal through SQL carries the rule's
  message; an error at a `.` is placed just past it, and a table named twice
  without an alias where its alias would be. Verified against the live 12.0
  engine under Wine (`fixtures/sql/links.json`, `tools/ace/sql/links.txt`).

- **Binary CDX orders whose bitmap takes more than one node are now built.**
  Creating or reindexing a binary order (`DELETED()` or another logical
  expression, `ADS_BINARY_INDEX`) on a FoxPro table of more than 3,967 records
  failed with `AE_NOT_IMPLEMENTED`; tflip now writes the original engine's
  bitmap leaves (3,968 records each, linked, first record big-endian) under an
  interior root of (last record, leaf) entries without keys, taking a reindex's
  nodes and its unused one after them as the engine does. Reading such an order
  is fixed as well (its interior entries have no key byte, and its leaves'
  first record is big-endian, as is the one maintenance now writes when a
  bitmap gains a leaf), and a binary order's key count follows the records
  appended since the table was opened. Verified byte-for-byte against the
  original engine (`fixtures/write/cdx_bitmaps`: build, appends into a fourth
  leaf, deletes, recalls, changes, reindex, and a root bitmap that splits), and
  the engine reads and maintains the files tflip writes. A bitmap of more than
  62 leaves (over 246,000 records) is still not built.

- **`TOP n PERCENT` and the `RAND([seed])` scalar function are now supported.**
  `TOP n PERCENT` keeps that share of the rows, rounded up and exactly (7% of 100
  rows is 7), after `DISTINCT` and `GROUP BY`, in each `SELECT` of a `UNION`, a
  derived table and a subquery; 0% keeps none, 100% and more keep all. As in the
  engine, `PERCENT` takes no `START AT`, a `TOP` count keeps its low 32 bits
  (`TOP 4294967297` is `TOP 1`), a `TOP` not followed by a number is a column
  name, `AT` is no alias, and `SHOW PLAN` has no step for `TOP`. `RAND()` is a
  `DOUBLE` from the connection's own generator, which reproduces the engine's
  numbers exactly: L'Écuyer's combined generator with the engine's 30-bit states,
  learnt black-box from its outputs. A seed restarts it (truncated to a 32-bit
  integer, of which 30 bits count), `RAND(0)` seeds from the clock, a NULL seed
  gives NULL, and every connection starts in the same state. A `WHERE` with
  `RAND` makes a static cursor. Verified against the live engine
  (`fixtures/sql/percent.json` and `funcs.json`, `tools/ace/sql/percent.txt` and
  `funcs.txt`).

- **`sp_GetColumns` describes a dictionary's views, and the dictionary
  procedures follow its links.** On a dictionary connection `sp_GetColumns` now
  lists a view's columns as the cursor its `SELECT` opens types them, with the
  nullability the original engine works out for each (a table column's own, even
  through an outer join; an expression nullable when a column in it is, but `IS
  NULL`/`CASE`/`IIF` always, `COUNT`/`EXISTS` never, other aggregates always; a
  union's first `SELECT` deciding), and a table column's dictionary comment,
  default and options carried through (`FIELD_OPTIONS` is now the dictionary's
  for tables too). Both procedures list a link's tables (not its views) under the
  link's name where the link sits in the object order, take the catalog argument
  (`''` the dictionary's own, else a `LIKE` pattern over link names and
  `system`) and, for `sp_GetTables`, the table type (one type, any case; `SYSTEM
  TABLE` lists the system tables alone, `TABLE`/`VIEW` show the matching rows
  under the numbers they have among all). Like the original, `sp_GetColumns`
  fails with 5018 when the pattern takes in a system table and with 5112 on a
  view reading a `SELECT` in its `FROM` (see `docs/ADS_BUGS.md`). Along the way:
  a `SELECT` can read a dictionary view in its `FROM`; a static cursor's double
  column shows 15 decimals, as the original's does; `CHAR + CHAR` stays `CHAR`;
  `AdsDDCreateLink` wraps the link password with the key the connection's login
  recovered (it failed as not implemented on a dictionary with an administrator
  password) and keeps a `..\` path as given; and `AdsDDAddView` refuses a
  statement too long for the dictionary record with `AE_NOT_IMPLEMENTED` instead
  of aborting. Verified against the live engine
  (`fixtures/sql/spview.json`, `tools/ace/sql/spview.txt`).

- **SQL numbers with an exponent are read as the original engine reads
  them.** The original gathers the digits in a double and then multiplies or
  divides by 10 once per power, rounding each time, so `SELECT 1e30` is
  9.9999999999999988e29 (`999999999999999880000000000000.0`) and
  `123.456e0` 123.45599999999999; tflip read them exactly. Probed on about 90
  literals (`tools/ace/sql/sql.c`). The text of a Double value was already
  the original's (its exact 17 digits, checked on about 2000 values with the
  new `tools/ace/fields/doubles.c`), but for a tie at the 18th digit, which
  the original rounds its own way and tflip, on purpose, the ordinary way
  (see `docs/DIFFERENCES.md`).

- **A setter's failure has the original engine's details in its error
  text.** `AdsGetLastError` after a failed `AdsSetString`, `AdsSetField`,
  `AdsSetDate`, ... now names the field as the table has it and, for some
  codes, the reason, exactly as the original engine does:
  `Error 5066:  The requested operation is not legal for the given field
  type.  Expected date field. Field: "N"`, `Character value too long.`,
  `Not a valid logical value.`, `Must be from 1 to 28.`, `Date won't convert
  to julian.`, `ADS_MONEY is available only with ADT tables.`, ... The write
  fixtures' replay now compares the error text of every `set` command
  (about 3500 of them failing in the fixtures, all matching). The protocol
  version is now 66 (`Error::InvalidDay` carries the month's days).

- **Dates are read from text as the original engine reads them, separators
  or not.** In a date format with separators (`MM/DD/CCYY`), the engine
  leaves out spaces and then skips exactly one character after the month and
  the day, whatever it is: `1108/2017` is 2017-11-08, `01011900` 1900-01-11,
  `11 8 2017` 1907-11-20, `11/082017` 0017-11-08 (a year of three or four
  digits is that year), while `11082017` has the day 82. tflip refused all
  of these. A wrong month fails with `AE_INVALID_MONTH`, a wrong day with
  `AE_INVALID_DAY`, the year 0 with `AE_INVALID_DATE`. This is the reading of
  Date setters, seek keys and `CTOD`; 157 texts in five formats were probed
  (`fixtures/fields/dates.tsv`, `tools/ace/fields/dates.c`). The date part of
  a timestamp text is still read as before (see `docs/KNOWN_ISSUES.md`).

- **Permissions on publications and subscriptions.** `AdsDDGrantPermission` and
  `AdsDDRevokePermission` now grant and revoke `ALTER` and `DROP` on a
  publication or subscription, `AdsDDGetPermissions` reads them back, and
  `system.permissions` / `system.effectivepermissions` list publications and
  subscriptions (after the links, before the users). Articles have no
  permissions, as in the original (5158 on a grant, 5136 on a read). Probing
  them showed corners every object type shares, now matched too: a user's
  inherit bit is stored — granted with a user's first permission, revocable and
  grantable on its own, not put back by a later grant — and a user inherits its
  groups' rights only while it holds it; `ADS_PERMISSION_ALL_WITH_GRANT` is
  accepted; a revoke carrying `ADS_PERMISSION_WITH_GRANT` takes away only the
  grant option, and a plain one the grant option too; a revoke with nothing
  granted still records the permission; a per-column slot left empty is dropped;
  and the error codes for no rights, `CREATE`, a grant on oneself and an object
  type with no permissions (5158, where tflip said not implemented). Verified by
  replaying 368 recorded engine calls and four `system.permissions` dumps
  (`tools/ace/dd/pubperm.c`, `fixtures/dictionary/pubperm.json`) and against 15
  golden dictionaries byte for byte, the enciphered rights compared deciphered,
  and both ways: the same probe run through tflip's own `ace64.dll` gives the
  engine's results. Through the DLL, a refused grant or revoke now returns its
  code (5132, 5136, 5158) as the original does, not SQL error 7200, and a null
  object name is an unknown object (5132), not an invalid pointer. SQL
  `GRANT`/`REVOKE` and object type 0 (any kind, by name) are still not run (see
  `docs/KNOWN_ISSUES.md`).

- **`AdsConvertTable` now converts every field type, from and to every table
  type.** Clipper, FoxPro and VFP tables convert too (before, only an ADT
  table and a FoxPro one of the common types did), to their own kind (fields
  kept as they are, VFP flags and AutoInc state included) and to the others;
  General and Picture fields convert to their own kind of table only, and an
  ADT table's legacy ADS VarChar is read and written. The probed rules: a
  Double or CurDouble becomes a Numeric 15 digits wider than its decimals (up
  to 30) in a Clipper or FoxPro table, a Character wider than 254 a memo, a
  name longer than ten characters `FIELD<n>`; a variable-length field of an
  ADT table is as long as its source is stored; an empty value is set empty
  and a NULL one NULL (blank in a Clipper or FoxPro table); a deleted record
  stays deleted in a DBF table and is left out of an ADT one, which also gives
  each record a RowVersion of its own; text memos are written after a
  record's other values. A value its field cannot hold ends the conversion
  with the engine's error (5179), the record written as far as it got, and a
  Clipper General or Picture value with 7019; a file of the target's name is
  overwritten. Probed with two new scenarios, `converts_dbf` and
  `converts_values`, verified byte for byte, locally and through tflipd.

- **Full-text plans and FIXED indexes read as the original engine does.**
  `SHOW PLAN` of a `CONTAINS` among other predicates now shows the original's
  AOF: each `CONTAINS` a term its full-text order optimizes fully, partly (a
  phrase, `NEAR`, or a word as long as the index's longest word, whose keys are
  cut to that length) or not at all (a `NOT` of a part the order only narrows),
  with the records the order finds, the details listed and looked up in the
  original's order, and nothing left to evaluate where the orders found no
  record. A static cursor (`SCORE`, `{static}`) shows its `EVALUATE` over an AOF
  of its restrictions (`CONTAINS(body,"card") = 1`, ordered and warned about as
  the original does), and a sort on values it does not show `<Using Bookmark>`.
  A `SCORE` anywhere (`WHERE`, `ORDER BY`) makes the cursor static, as there. A
  FIXED (not maintained) full-text index now answers `CONTAINS` from its keys
  wherever the original's AOF does (and `SCOREDISTINCT` always), so rows changed
  since it was built match as there; a live cursor of full-text searches counts
  the records the orders find. The `CONTAINS` parser drops a noise or too-short
  word with the operator that joins it (`medical near x` is `medical`), the
  scores count a phrase's words wherever they stand, a full-text error in a live
  cursor's `WHERE` is the original's own error rather than an SQL one, and a
  `SELECT` takes a statement number as a plan does. Verified against
  `fixtures/sql/fts_plans.json` (`tools/ace/sql/fts.txt`, 339 statements on new
  engine-made tables `fx`, `px`, `qx`); protocol version 67.

- **`tflipd` keeps closed tables open for reuse (`AdsCacheOpenTables`,
  `AdsCacheOpenCursors`, `AdsCloseCachedTables`).** These were accepted and
  ignored; a connection to `tflipd` now keeps up to N tables its client closed
  (none by default) and the tables of its closed SQL statements (25 by default)
  open on the server, so opening them again does not open and read the file
  again. As probed on the original server and its local server
  (`tools/ace/cache`): the cache is per connection, the oldest closing first; a
  kept table shows in `AdsMgGetOpenTables` but holds no lock, and lets go of the
  file for an exclusive open by any connection; `DROP TABLE` closes every
  connection's kept tables; `AdsCloseCachedTables` and `AdsCloseAllTables`
  close kept tables but not statements' tables; lowering the table size closes
  what is past it at once, lowering the cursor size does not; disconnecting
  closes all. A file only a cache holds is reused only while it is unchanged on
  disk (same file, length and write time), so a table another program replaced
  or rewrote is read afresh. Exclusive and encrypted tables are never kept.
  Replayed against the original server's answers (`fixtures/cache/remote.txt`,
  `tflip-ace` `open_cache`). Protocol version 68. Two corners are left (see
  `docs/KNOWN_ISSUES.md`): the local server keeps nothing (the original's does
  cache), and a statement lets go of its table when its cursor closes rather
  than when it closes. Also fixed along the way: marking a DBF table's
  structural index now updates the open file's header too, so a later open
  sharing it sees the index.

- **More AOF parts are looked up in the index orders.** `<>` (the order's
  records but those whose key is the value's), a comparison of the first field
  of an ADI key of several fields (`a = 'ab'` with an order on `a;b`), and text
  under an ADI order's dynamic collation are now found in the orders instead of
  by evaluating every record. Probing showed the original engine's results
  depend on its orders here, and tflip now gives the same: a table opened with a
  dynamic collation compares text in its expressions by the collation's weights
  (`name = 'APFEL'` finds `apfel`, `Apfel` and `apfelbaum`; `name = 'ss'` finds
  `ß`), with an order or without; `YEAR()` is optimized only by `=` through an
  order on the date (other comparisons are not, as `AdsGetAOFOptLevel` now
  reports), by any comparison through an order on `DTOS()` of it, and both find
  the year the value's whole part is (`YEAR(born) = 1990.5` shows the 1990
  records, as in the original engine; see `docs/ADS_BUGS.md`). Verified against
  the original engine under Wine (`fixtures/write/aofs_lookup`,
  `fixtures/write/aofs_collation`; `write.c` gained `exact` and `collation`).

- **NChar and NVarChar orders can be sought.** Their keys are ICU sort keys
  (`:en_US`), which tflip now builds as the original engine does, from a table
  of the collation elements of some 5,200 characters — Latin, Greek, Cyrillic and
  their extensions, punctuation, symbols, letterlike and fullwidth forms
  (`crates/tflip-storage/src/icu_root.txt`). The table was worked out black-box
  from the engine's keys of every BMP character (`tools/ace/seek/nkeys.py`,
  which describes the model: levels, common-weight compression, primary lead
  compression), and checked against its keys of 70,000 random strings without a
  difference. `AdsSeek`, `AdsSeekLast`, `AdsSetScope` and `AdsLookupKey` seek a
  text's whole sort key, as the engine does (`ab` does not find `abc`), without
  its trailing spaces unless it is all spaces; `ADS_WSTRINGKEY` text is taken as
  Unicode. `AdsExtractKey` gives such an order's keys, and finding the current
  record seeks its key. Text with another character still fails with
  `AE_NOT_IMPLEMENTED`, and tables with such orders still take no writes.
  Checked against the engine's seeks, scopes and keys (`fixtures/seek/nchar.json`)
  and keys (`fixtures/seek/nkeys.txt`).

- **Seeks and scopes in NTX and CDX orders weigh by the collation.** Under a
  static collation (`ANSI_CHAR_SET`, or `OEM_CHAR_SET` for an OEM table) the
  orders already walked and found keys by weight, but a scope still compared
  bytes, and `ADS_SEEKGT` and `AdsSeekLast` stepped past the sought key byte-wise
  — so under German a scope from `a` to `b` left out `ä`, and seeking after `a`
  skipped the `ä` words. They now work by weight too. And text sought in an OEM
  table (`AdsSeek`, `AdsSeekLast`, `AdsSetScope`, `AdsLookupKey`) is converted to
  the client's OEM code page first, as the original engine does, so `Äpfel` finds
  the `8E 70 66 65 6C` the table holds. Checked against the engine on some 1,200
  seeks, seek-lasts, scopes and walks in ADT, NTX and CDX orders under the German
  collations and the defaults (`fixtures/seek/`, from the new `tools/ace/seek`).

- **`sp_GetTables` and `sp_GetColumns` now reflect a dictionary connection.**
  On a data-dictionary connection the two procedures listed the folder's free
  tables; they now list the dictionary's own tables and views in object order
  (`TABLE`/`VIEW`, with the object's comment), followed by the system tables
  (catalog `system`, type `SYSTEM TABLE`), exactly as the original engine does.
  `sp_GetColumns` carries each column's dictionary default (`COLUMN_DEF`),
  comment (`REMARKS`) and nullability (a column is nullable only when its type
  allows it and the dictionary does not mark it `NOT NULL`). Verified
  byte-for-byte against the live engine on the native VM (`fixtures/sql/spdict.json`,
  `tools/ace/sql/spdict.txt`). Two corners are left (see `docs/KNOWN_ISSUES.md`):
  a dictionary's links are not followed, and a view's columns are not described.
  On a free-table connection both procedures are unchanged.

- **`sp_GetSQLKeywords` and the `system.systemgroups` / `system.articles`
  catalog views are now supported.** `EXECUTE PROCEDURE sp_GetSQLKeywords()`
  returns the engine's 91 reserved words as a one-column `Keyword` cursor, in the
  engine's own order (not quite alphabetical). `system.systemgroups` lists the six
  built-in groups (`DB:Admin`, `DB:Public`, `SERVER:Admin`, …) with their
  `Can_Alter_Permissions`/`Can_Modify_Membership` flags, and — like
  `system.objects`/`system.fts` — works on a free connection with no dictionary.
  `system.articles` is served as an alias of `system.publicationarticles` (the
  same eight columns), the name Advantage Data Architect actually queries. All
  three were verified byte-for-byte against the live engine on the native VM
  (`fixtures/sql/system.json`, `tools/ace/sql/system.txt`).

- **The `NEWIDSTRING(format)` expression function is now supported.** In record
  filters and index expressions the engine's `NEWIDSTRING` takes a format string
  and returns a new GUID written that way: `D`/`DELIMITED` (`8-4-4-4-12` hex),
  `B`/`BRACKETED` (the same in **square brackets**), `P`/`PARENTHESIS`,
  `N`/`NUMBERS` (32 hex, no separators), `M` (MIME Base64, 24 bytes) and
  `F`/`FILE` (URL-safe Base64, 22 bytes, no padding), case-insensitive. A missing
  argument is a parameter-count error (3402), a non-string one a type mismatch
  (3409), and an unknown format fails at evaluation with 4407 — all matching the
  live engine on the VM (`tools/ace/expr`). The documented `MIME` and `REGISTRY`
  long names are *rejected* by the engine itself, a documentation/engine mismatch
  recorded in `docs/ADS_BUGS.md`. The GUID value is random, so it is matched by
  shape; the format layouts are byte-exact. (This is the expression engine. The
  SQL scalar `NEWIDSTRING` takes the same formats but as an *unquoted word* —
  `NEWIDSTRING(M)`, not `NEWIDSTRING('M')` — and, unlike the expression engine,
  accepts the `MIME` long name and reports an unknown format as 2159 rather than
  4407; see the entry below.)

- **SQL `USER()`, `DATABASE()`, `NEWID()` and `NEWIDSTRING()` are now
  supported.** `USER` (with or without parentheses) folds to the connection's
  user — the dictionary login name, or the OS user of a free-table connection —
  as a `VARCHAR` of that name's length, exactly as the original engine prepares
  it (a real column named `USER` still wins). `DATABASE()` is the connection's
  database path (the folder of a free connection, the `.add` of a dictionary
  one) as a `VARCHAR(255)`. `NEWID()` returns a new `GUID`; `NEWIDSTRING()` the
  GUID as text, a `VARCHAR(38)` — the 36-character delimited form by default, or
  the layout an unquoted word gives (`NEWIDSTRING(M)` for MIME base64,
  `NEWIDSTRING(BRACKETED)`, …), matching the expression-engine formats above.
  The column types, lengths and the `DATABASE()`/login values were verified
  byte-for-byte against the live engine on the native VM (`fixtures/sql/funcs.json`,
  `tools/ace/sql/funcs.txt`); the GUIDs, being random, are matched by shape.

## v0.18

- **SQL `GROUP_CONCAT` is now supported.** The aggregate joins a group's text
  values into a `VARCHAR(1024)`, with the default comma separator or a
  `SEPARATOR 'str'`, optional `DISTINCT` (first occurrence kept), and an inner
  `ORDER BY` (`GROUP_CONCAT(name ORDER BY name DESC SEPARATOR '; ')`). NULLs are
  skipped and an all-NULL or empty group is NULL; a `CHAR` argument keeps its
  padding and a `VARCHAR` does not; a numeric or memo argument is rejected
  (2124), and a result past 1024 bytes raises "Data truncated" (2102) — all
  matching the engine, verified byte-for-byte on the native VM against both
  tflip's and the original's `ace32.dll` (`fixtures/sql/groupconcat.json`,
  `tools/ace/sql/groupconcat.txt`). `GROUP_CONCAT` over a case-insensitive
  (`CICHAR`) column is the one unmatched corner — see `docs/KNOWN_ISSUES.md`.

- **`AdsImageToClipboard` now copies the image to the Windows clipboard.** When
  tflip's `ace32.dll` runs inside a Windows process, an Image field's BMP is set
  on the clipboard as `CF_DIB` — the BMP's bytes with its 14-byte
  `BITMAPFILEHEADER` stripped — and Windows synthesises `CF_BITMAP`/`CF_DIBV5`
  from it, byte-for-byte as the original engine does. A field of another type
  still fails with `AE_INVALID_FIELD_TYPE`, an unknown field with
  `AE_INVALID_FIELDNAME`, and data that is not a valid BMP with 5091 ("Invalid
  bitmap type." / "Missing BITMAPFILEHEADER.") or 5001 — each verified on the
  native Windows VM against both tflip's and the original's `ace32.dll` (probe in
  `tools/ace/clipboard`). A non-Windows build (tflipd on Linux) has no clipboard,
  so a valid image reports `AE_NOT_IMPLEMENTED` as before. This was the last of
  the two native-Windows functions (M10); `AdsFindServers` landed earlier.

- SQL **`UPPER` and `LOWER` now fold case by the static collation**, as the
  original engine does, where before they folded ASCII only. Under
  `ANSI_CHAR_SET=German`, for example, `UPPER('ä')` is `'Ä'` and `LOWER('Ö')` is
  `'ö'`; under the default collation nothing changes (ASCII folding). Each
  Windows-1252 byte goes through the collation's own case table (its display
  character — the SQL functions return the folded text, so they use the
  unweighed table, unlike the expression evaluator's `UPPER`/`LOWER`, which
  weigh for comparison). Probed black-box (`tools/ace/sql/collation.txt` with
  `run_collation.sh`) and verified against the engine golden
  (`fixtures/sql/collation_case.json`, every byte 32–255). The SQL evaluator's
  string *comparisons* under a collation are still to come. OEM (DBF) tables use
  the ANSI collation here, as the per-statement environment is connection-level.

- A client of `tflipd` now reads a **whole record's fields in one request**
  instead of one request per field. The first `AdsGet*` of a field after a move
  fetches every non-memo field of the current record at once
  (`read_fields`), and the rest are answered from that cached row until the
  cursor moves, a record changes, or a formatting setting (date format,
  "exact", the epoch, shown-deleted records, a collation) changes — any of
  which drops the cache, so a read never returns a stale value. Memo, binary
  and image fields stay fetched on demand (they live in the memo file), as do a
  DBF `LegacyVarChar`'s long values. The server still decides each field's
  value, so every per-field result (and error) is exactly what reading it alone
  would give. Reading five columns of a row now costs one round trip, not five
  (`tflip-ace`'s `round_trips::a_row_is_read_once_and_dropped_on_a_move`). The
  protocol version is raised for the new request.

- Expression **string comparisons** (`=`, `<>`, `<`, `>`, `<=`, `>=`, and
  `INLIST`) now **weigh their bytes by the static collation**, as the original
  engine does, where before they compared Windows-1252 bytes. Under
  `ANSI_CHAR_SET=German`, for example, `ä` sorts just after `a` (and `ä = a` is
  false, as their weights differ), `ß` sorts near `s`, and a lowercase letter
  weighs before its uppercase. The `AdsSetExact` padding is weighed too, and a
  case-insensitive (CICharacter) operand folds case through the collation first;
  `==` and `$` stay exact byte operators. Under the default collation the
  weights are the bytes themselves, so nothing changes. This covers the xBase
  expression evaluator — filters, AOF conditions and index expressions; the SQL
  evaluator's comparisons and seeks under a collation are still to come. Probed
  black-box (`tools/ace/expr/compare.txt`) and verified against the engine
  golden (`fixtures/expr/compare.json`, >2000 comparisons).

- Opening a table whose index keys a **different static collation** weighed is
  now refused with `AE_INDEX_COLLATION_MISMATCH` (5175), as the original engine
  does, where before tflip opened it and navigated by the wrong order. An ADI
  file records a CRC-32 of its collation's byte-to-weight table in its header
  (0x14) — tflip already wrote this when creating indexes; it now reads it on
  open and compares it with the connection's collation (the default being the
  identity table's CRC), refusing the table when they differ. NTX and CDX keys
  are raw, not weighed, so they are not affected. Confirmed against the engine
  under Wine (`tools/ace/expr/openmismatch.c`: 5175 under the default collation,
  0 under `ANSI_CHAR_SET=German`). The protocol version is raised so a server
  cannot send the new error to an older client.

- The **enforcement toggles** `AdsDisableUniqueEnforcement` /
  `AdsEnableUniqueEnforcement` and `AdsDisableAutoIncEnforcement` /
  `AdsEnableAutoIncEnforcement` are acted on now, where before they were taken
  and ignored. With **unique** enforcement off a connection's write adds a key an
  ADI-unique or CDX-candidate order already has instead of refusing it with
  `AE_KEY_NOT_UNIQUE` (7057); with **AutoInc** enforcement off a client stores
  its own value in an AutoInc field of an ADT table, and the table's next value
  is kept past it so later fills do not collide. Both are carried per connection
  and forwarded over `tflipd` (a new protocol entry). Probed black-box
  (`tools/ace/write/enforce.c`) and verified against the engine golden
  (`fixtures/write/enforce`). Known gap: a CDX-candidate duplicate inside an
  explicit transaction stays enforced (ADI orders are not affected).

- `AdsSetStringFromCodePage` now converts from the four **double-byte code
  pages** — 932 (Shift-JIS), 936 (GBK), 949 (UHC) and 950 (Big5) — which were
  stubbed before (`AE_NOT_IMPLEMENTED`). The engine converts these through its
  bundled ICU (`aicu32.dll`), so the mappings are its own, not the host's NLS;
  they were probed byte-pair by byte-pair through the real DLL under Wine
  (`tools/ace/codepage`, `fixtures/codepage/dbcs.json`) and baked into a table
  the new `dbcs` module reads. A lead byte takes the following byte as its
  trail to make one character and always consumes two bytes; a lead byte with
  no trail, an unmapped pair, or an undefined byte becomes U+FFFD — exactly as
  the probe recorded. The single-byte half-width katakana of 932 and the
  engine's control-character quirks come through unchanged.

## v0.17

- The storage layer now **writes** AES-encrypted tables, byte for byte as the
  original engine does: the table header's AES markers and 600-byte crypto block
  (type, nonce, IV counter, PBKDF2 salt and descriptor IV), the verifier derived
  from the password, the field descriptors enciphered as one AES-CTR stream (with
  fields laid out from record offset 13 after the clear 8-byte IV), records and
  memo values enciphered on write (each drawing a fresh IV from a shared,
  persisted counter), and index tree pages enciphered (`[data][IV]` with the
  leaf-displacement the reader undoes), both dictionary-bound (encrypted
  descriptors) and free (clear descriptors, no binding). The format is verified
  byte for byte against the committed `fixtures/aes` tables — including a free
  table captured from an OpenSSL-shipping patched engine build, under which AES
  runs under Wine (the stock build faults 7160) — and by tflip's own reader.
- A client can now **create a free (non-dictionary) AES table** through the ACE
  API, as the original engine does: a connection whose connect string carries
  `EncryptionType=AES128` or `AES256` lays a created table out for AES (the full
  structure, but unarmed — no key yet), `AdsEnableEncryption` with the password
  arms it (fills the crypto block, derives and checks the verifier, sets the
  ENCRYPTED flag, and enciphers later writes), and `AdsEncryptTable` is the no-op
  the engine makes it on a table already armed. The same `AdsEnableEncryption`
  call supplies the password to read an already-encrypted table instead of
  re-encrypting it, and `AdsIsEncryptionEnabled`/`AdsIsTableEncrypted` report an
  AES table truthfully. Verified end-to-end both ways against the patched engine
  — tflip creates and fills a free AES table and the engine reads its records,
  and the engine's own free AES table reads back through tflip, with identical
  record encipherment. Creating a *dictionary-bound* AES table through the API
  still awaits the strong-encryption dictionary format (read and write).
- A connection to `tflipd` now takes a **per-request timeout**, so a server
  that stops answering no longer blocks a client forever (the original engine's
  client blocks indefinitely). It is off by default and set in seconds through
  `[connection] request_timeout` in `tflip.ini` or `TFLIP_REQUEST_TIMEOUT`;
  the connect timeout (5 s) is configurable the same way (`connect_timeout` /
  `TFLIP_CONNECT_TIMEOUT`). A read or write that times out drops the
  connection, and the call fails with the engine's "no connection" error.
- `AdsConnect101`'s **TLS communication settings** are parsed now. `tflipd`
  always speaks TLS 1.2/1.3 and trusts a server by its certificate's
  fingerprint, so a connect string asking for a non-TLS transport
  (`CommType=TCP_IP`, `UDP_IP` or `IPX`) is refused with `AE_INVALID_OPTION`
  rather than silently given TLS; the cipher list, certificate, common name and
  FIPS mode are logged on the `remote` channel and accepted, since they do not
  change that the connection is encrypted.
- `AdsDDMoveObjectFile` now moves a table's files (its `.adt`, its `.adm` memo
  if it has one, and the `;`-separated index files named in the call) or a single
  index file to a new directory, updating each moved object's stored relative
  path (`.\<dir>\<file>`), as the original engine does — rather than failing as
  not implemented. An unknown table or index file fails with `5132`, and a
  destination that does not exist, or one where the file is already present,
  with `5020` (the engine's wording). Verified by a write/read round-trip and
  end-to-end against tflip's own DLL under Wine. (The on-disk behaviour was
  derived black-box; see [`docs/ADS_BUGS.md`](docs/ADS_BUGS.md).)
- `AdsDDCreateRefIntegrity` / `…62` now check the same preconditions the
  original engine does, with its own codes and messages (probed black-box; see
  [`docs/ADS_BUGS.md`](docs/ADS_BUGS.md)): a missing parent or child table, or a
  missing child index, fails with `5132` (each with its own wording); a parent
  table with no designated primary key fails with `5131`; and a parent tag that
  is not that primary index fails with `5143`. Earlier tflip skipped these and
  would write a rule the engine rejects.
- `AdsDDSetTableProperty` with `ADS_DD_TABLE_PRIMARY_KEY` now designates a
  table's primary key — storing the named index's id in the table's `Info1`, as
  the original engine does — which a referential-integrity rule needs set before
  it can reference the table. Checked by a write/read round-trip.
- `AdsDDSetObjectAccessRights` now sets an accessor's rights on an object,
  rather than failing as not implemented. The object's type is inferred from its
  name (the call carries none), and the access string is read letter by letter
  as the original engine does — probed against the real engine (`tools/ace/dd/setobj.sh`):
  `R` is read, `W` the write rights (update, insert and delete together), `X`
  execute and `I` inherit, and every other letter is ignored. The rights are
  validated against the object's type (a right the type cannot carry is refused
  with `5158`, as a grant is) and then written to the accessor's Permission
  object, replacing whatever was there (unlike a grant, which merges). An unknown
  object fails with `5137` and an unknown accessor with `5132`. The Permission
  object is the grant path's (verified byte for byte), so the result is checked
  by a write/read round-trip.
- `AdsDDRemoveTrigger` now removes a trigger: it soft-deletes the Trigger object
  and recomputes its table's aggregate trigger-type and option slots from the
  triggers that remain (the exact inverse of creating one), rather than failing
  as not implemented. The original engine's own `AdsDDRemoveTrigger` fails with
  `5132` for a trigger it wrote and crashes on one made with SQL `CREATE
  TRIGGER`, so there is no behaviour to match byte for byte; the removal is
  verified by a tflip round-trip (create, remove, check the object is gone and
  the table aggregates fell to the remaining triggers' bits). An unknown trigger
  is refused with `5132`.
- `AdsDDModifyLink` now rewrites a dictionary link's linked-dictionary path,
  user name, password (wrapped with the dictionary key) and options, rather than
  failing as not implemented. The original engine's own `AdsDDModifyLink` cannot
  act on a link it wrote — it returns `5132` ("no such named link"), as its
  `AdsDDDropLink` and `AdsDDRemoveTrigger` do — so there is no golden to match
  byte for byte; the result is checked by a tflip round-trip (create, modify,
  read the changes back). An unknown link is refused with `5132`, and a
  dictionary whose key tflip cannot recover (a non-empty administrator password)
  fails as not implemented, as with `AdsDDCreateLink`.
- `UPPER()` and `LOWER()` in an expression now fold case by the static collation,
  as the original engine does, rather than only the ASCII letters: under a
  non-default `ANSI_CHAR_SET` (or `OEM_CHAR_SET` for an OEM table) each byte is
  folded through the collation's case table and weighed, so `UPPER` of an
  accented letter follows the language's rules. Under the default collation it is
  plain ASCII folding, unchanged. Probed on the `charset_german` fixture under
  `ANSI_CHAR_SET=German` (`tools/ace/expr/run_case.sh`) and checked against
  `fixtures/expr/case.json`. Collation-aware string *comparisons* (and SQL
  `UPPER`/`LOWER`) are still to come.
- `AdsSetStringFromCodePage` now converts from every single-byte code page the
  original engine accepts — the Windows and OEM pages 437, 500, 737, 775,
  850–869, 874/875, 1026 and 1250–1258 — using the engine's own byte-to-Unicode
  tables (which differ from Windows' standard ones), rather than only 1252 and
  437. A code page the engine has no converter for fails the same way it does
  (`1565`); the double-byte pages it also converts (932/936/949/950) are not
  done yet (`AE_NOT_IMPLEMENTED`). Probed byte by byte through the real engine
  (`tools/ace/codepage`) and checked against `fixtures/codepage/`; the 1252 and
  437 tables match tflip's existing ones byte for byte.
- An OEM Clipper or FoxPro table (one opened with `ADS_OEM`) now orders its NTX
  and CDX keys by the OEM static collation (`OEM_CHAR_SET` in `adslocal.cfg`,
  read from `extend.chr`) rather than the ANSI one, so it navigates and seeks in
  the order the original engine records; an ANSI table keeps using the ANSI
  collation (`ANSI_CHAR_SET`, `ansi.chr`). Checked against the `charset_german`
  NTX and CDX fixtures, which hold the same words in an ANSI table (`ca`) and an
  OEM one (`co`). As before, the collation files are read where a client has
  them and are shipped with neither.
- `AdsConvertTable` now converts the extended field types too, as the original
  engine does: LongInt, RowVersion, GUID, Raw, VarBinary, VarChar, NChar,
  NVarChar, NMemo, ModTime and the compact (short) date. To a Clipper or FoxPro
  table they widen (numbers to Numeric, a GUID to a 36-character string, raw and
  variable-length data to Character, a Unicode memo to an NMemo kept as UTF-16,
  ModTime split into a date and a time like a TimeStamp); a Visual FoxPro table
  keeps the Fox variable-length and Unicode types with their binary and null
  flags. Probed with a new `converts_ext` scenario, verified byte for byte.
- An index order built on an expression now reports the key type of its result,
  as the original engine does: `n + 1` is numeric, `date + 1` is a date, `n > 0`
  is logical (before, every non-character expression order was reported as a
  string). `AdsGetKeyType` is correct for them, and a numeric or date expression
  order is now sought by a proper numeric key rather than raw bytes, so seeks on
  it find their records. Checked against the new `expr_keys` fixture and
  `fixtures/expr/keytype.json`.

## v0.16

- A data dictionary's referential integrity rules are now enforced, as the
  original engine does: inserting or updating a child row whose foreign key has
  no matching parent is refused (7076 through the table API, wrapped as the AQE
  error 7200 through SQL), and updating a parent's key or deleting a parent row
  applies the rule to its children — RESTRICT refuses while a child points at the
  key, CASCADE deletes the children or re-points them at the new key, SET NULL
  clears their foreign key, and SET DEFAULT sets it to the column's default
  (which must itself be a valid parent key). A NULL foreign key is allowed. The
  rules apply on direct `AdsAppendRecord`/`AdsWriteRecord`/`AdsDeleteRecord` and
  on SQL `INSERT`/`UPDATE`/`DELETE` alike; a RESTRICT refusal of a table-API
  delete leaves the row and returns success, as the original does, where SQL
  surfaces the error. Cascades run with the table owner's rights. Checked against
  `fixtures/dictionary/ri.json`.
- A data dictionary that verifies access rights (`VERIFY_ACCESS_RIGHTS`) now
  enforces them on the connected user, as the original engine does: a table the
  user has no read right on cannot be opened (error 7079), and an insert, update
  or delete the user lacks the right for is refused (5054 through the table API,
  wrapped as an AQE error, 7200, through SQL). Rights roll up through the user's
  groups and the built-in groups, and a grant on any column counts toward the
  table (the engine has no per-column read or write enforcement); `ADSSYS` and a
  dictionary that does not verify rights are unrestricted. Trigger bodies run
  with the table owner's rights, not the invoker's. Over `tflipd` the server
  enforces for the authenticated user. Checked against
  `fixtures/dictionary/rights.json`.
- Logging in to a data dictionary checks the password, as the original engine
  does: `ADSSYS` is authenticated by the administrator password that recovers
  the dictionary key, every other user by the login secret stored for it; the
  user name matches whatever its case, the password exactly, and a wrong, empty
  or missing password for a user that needs one fails with error 7078. Over
  `tflipd` the password is forwarded to the server (through TLS), so a remote
  connection is authenticated too. Checked against `fixtures/dictionary/login.json`.

## v0.15

- Full-text search, second part: `CONTAINS` and `SCORE` run in SQL. `CONTAINS`,
  `SCORE`/`SCOREDISTINCT` and the ordinal `SCORE(N)` work in the select list, the
  `WHERE` and `ORDER BY`, over a field's full-text index (its parsing options
  read from the `.adi`), `CONTAINS(*, …)` over every full-text field, and with a
  fallback for a field with no index; the boolean operators, phrases, `NEAR` and
  wildcards, the native error codes, the NULL-field rules and per-index options
  (case-sensitive, protect-numbers, custom character sets) follow the original
  engine, checked against `fixtures/sql/fts.json`.
- `AdsGetFTSIndexInfo` and `AdsGetFTSIndexInfoW` report a full-text order's
  parsing options: the indexed field, the delimiter, noise, drop and conditional
  character sets, the word-length bounds and the option flags.
- `SHOW PLAN` of a live `CONTAINS` cursor shows its AOF over the full-text order
  (`AOF SCAN (LIVE)` with the index, or `TABLE SCAN (LIVE)` for a field with no
  index or a condition the index cannot answer).
- `AdsGetKeyFilter` builds the filter that identifies the current record by a
  unique index, byte-for-byte as the engine does: the unique index whose tag
  sorts first (or the table's AutoInc field, else error 5041), each key field as
  `field=value` joined by ` AND `, with the engine's value formats (text quoted
  but unescaped, doubles in `%E` form, Money with four decimals, dates as
  `datetime'...'`); the OData option joins with commas and uses a `T` and the
  `M`/`L` type suffixes.
- Dictionary property setters for users, groups and stored procedures:
  `AdsDDSetUserProperty` (comment, user-defined property, require-old-password),
  `AdsDDSetUserGroupProperty` (comment, user-defined property) and
  `AdsDDSetProcedureProperty` (comment, parameter lists, DLL name and function,
  invoke option and the script, narrow and wide). Checked byte-for-byte against
  the engine's goldens.
- Table encryption writing, the RC4 (Advantage proprietary) scheme:
  `AdsEnableEncryption` sets a table's password (writing the header's password
  check and type); `AdsEncryptTable`/`AdsDecryptTable` encipher or decipher every
  record and memo value and set or clear the table's encrypted flag; the
  record-level `AdsEncryptRecord`/`AdsDecryptRecord` do one record; and
  `AdsDisableEncryption` drops the session password. Writes to an encrypted table
  (appends, updates, deletes) are enciphered — records and memo values; the index
  is left in the clear, as the original engine leaves it. `AdsIsEncryptionEnabled`,
  `AdsVerifyPassword` and `AdsIsRecordEncrypted` now report truthfully. The on-disk
  lifecycle was derived black-box from the original engine and is checked
  byte-for-byte against its goldens (`fixtures/write/encryption`). AES encryption
  writing (creating an AES table) is still to come; reading both schemes is done.
- Replication objects: the data-dictionary functions for publications, articles
  and subscriptions are implemented (`AdsDDCreate/Delete/Get/SetPublicationProperty`,
  the article functions including `AdsDDCreateArticle100`, and the subscription
  functions). A publication holds its options; an article is named by the
  published table's object id and keeps the filter (with a field-number "short"
  form), the row-identifying columns and an include/exclude list; a subscription
  holds the target, user, enciphered password and connection string, the
  forward/enabled/paused flags, the queue and the communication-type options, and
  — as the engine does — synthesizes its replication queue table (a fixed 17-field
  table with two filtered index orders and its three files). Deleting a
  subscription removes the whole queue table and its files. The on-disk format was
  derived black-box from the original engine and is checked byte-for-byte against
  its goldens (`fixtures/dictionary/ops/{pub,art,sub}*`); the enciphered secrets,
  which carry a per-dictionary key, are verified by round-trip. Running
  replication itself (applying the queue, forwarding updates) is not included.
- Triggers fire: a dictionary's table triggers run around the SQL that changes a
  table — BEFORE each affected record's change, AFTER it, or INSTEAD OF it (the
  change is then skipped). A trigger body runs through the script interpreter and
  reads the affected record as the pseudo-tables `__new` (after an insert or
  update) and `__old` (before an update or delete), in `FROM` or as
  `__new.col`/`__old.col`, and the record number as `::stmt.TrigRecNo`; it may
  call user-defined functions, and its own DML fires further triggers. Checked
  against `adsdump` on the `psm` fixture (BEFORE/AFTER INSERT, AFTER UPDATE, and
  an INSTEAD OF DELETE that updates the row instead). CONFLICT triggers and the
  NO MEMOS / NO TRANSACTION options are not acted on yet.
- User-defined functions run in expressions (`sq(3)`, `util.pad(id, 4)`,
  package-qualified): the compiler resolves a call that is not a built-in to a
  function of the connection's dictionary, typed by its declared return type, and
  the evaluator runs its SQL/PSM body (parameters read by their plain names).
  Functions that run SQL against tables are not supported yet. Checked against
  `adsdump` on the `psm` fixture.
- SQL scripts: a dictionary's script stored procedures run (`EXECUTE PROCEDURE
  name(args)`), not just the built-in `sp_*` procedures. The script language is
  parsed (`tflip_sql::parse::parse_script` into a sequence of `ScriptStmt`: the
  `;`-separated statements, `DECLARE`, variable assignment, `IF … THEN … ELSE …
  END IF`, `WHILE … DO … END WHILE`, `BEGIN … END` blocks, `RETURN`, and the
  ordinary SQL it runs) and run by an interpreter (`tflip_sql::script`): it keeps
  the procedure's parameters (read in the body as `_name`) and its `DECLARE`d
  locals, runs the control flow, and hands the body's SQL statements — with the
  variables substituted in as literals — back to the engine to run on its tables.
  A procedure's results are the rows its body writes to `__output` (whose columns
  are the output parameters), or, for a `CURSOR VARYING` procedure, the result set
  of the `SELECT` the body runs. Checked against `adsdump` on the `psm` fixture
  (`numbers`, `pick`, `add_item`). External (`.aep`/DLL) procedures, triggers (so
  a procedure's trigger side effects), user-defined functions in expressions, and
  the pseudo-tables `__input`, `__new` and `__old` are still to come.
- `AdsDDCreateLink` adds a link to another data dictionary: a LinkedDD object
  under the Database (its `Info1` the options) holding the user name, the link
  password (`wrap`ped with the dictionary key) and the path to the linked
  dictionary relative to this one's folder — written as the original engine
  writes it, byte for byte (the deterministic rest; the wrapped password, keyed by
  the per-dictionary key, is masked like the other secrets and verified by
  unwrapping it), cross-checked both ways through tflip's own DLL. The key is
  recovered from the connection's administrator password (empty for a dictionary
  tflip writes); without it the link fails as not implemented. An empty or
  already-used alias is refused with 5132. `AdsDDDropLink` removes a link by name
  or path (soft-deleting its object; the original engine's own `AdsDDDropLink`
  fails with 5132, so the removal is checked for self-consistency).
  `AdsDDModifyLink` stays a stub.

## v0.14

- Full-text search, first part: the `CONTAINS` query engine (`tflip_sql::fts`).
  A tokenizer splits text into the word list a full-text index would hold — drop
  characters removed, split on the index's delimiters, conditional characters
  trimmed at the edges, folded to lower case unless case-sensitive, and noise or
  too-short words left out — and a parser turns a `CONTAINS` condition into a tree
  of words (with leading/trailing `*` wildcards), phrases, `AND`/`OR`/`NOT` and
  `NEAR`(n), with the engine's precedence (NEAR, then NOT, then AND, then OR),
  right-associative NEAR, and its native parse errors (7094–7099, 7103). The
  matcher evaluates the tree against a record's words, NEAR and phrases counting by
  position. Verified against the engine's recorded answers for 80 of the
  `fixtures/sql/fts.json` cases (the committed FTS fixture tables are added too).
  Wiring `CONTAINS`/`SCORE` into SQL, reading an index's own parsing options, and
  creating full-text indexes are still to come.
- `AdsDDCreateRefIntegrity`, `AdsDDCreateRefIntegrity62` and
  `AdsDDRemoveRefIntegrity` add and remove a referential-integrity rule in a data
  dictionary: a Relation object under the Database (its `Info1` its own id)
  holding the parent and child table and index ids, the update and delete rules,
  and the no-primary and cascade error messages, with both tables' RI-graph slot
  pointing at it. The Relation record and the RI-graph slots are written as the
  original engine writes them — verified byte for byte against a full-engine
  dictionary golden (the one that holds a relation), and end to end through
  tflip's own DLL. An unknown table or tag, or a reused name, is refused with
  5132. (The 12.0 evaluation engine's own `AdsDDCreateRefIntegrity` fails with
  5131 or crashes, and SQL `ALTER TABLE … ADD FOREIGN KEY` crashes before it
  writes, so a fresh both-ways golden cannot be produced here; the format is
  taken from the committed full-engine fixture.)
- `AdsDDCreateTrigger` and `AdsDDCreateTrigger100` add a trigger to a data
  dictionary: a Trigger object under the table it fires on (its `Info1` the
  table), holding the event and trigger types, the container type, the container
  (a SQL script or an external library), the function name, the priority and the
  options, with its comment in the dictionary memo. The table's aggregate
  trigger-type and option slots are updated to take in the new trigger, and the
  dictionary version is raised to 4 (triggers need a newer engine) — written as
  the original engine writes it, byte for byte (the records, the memo and the
  rebuilt index), cross-checked both ways through tflip's own DLL. An empty or
  already-used name, or an unknown table, is refused with 5132.
  `AdsDDRemoveTrigger` stays a stub: the original engine's own
  `AdsDDRemoveTrigger` fails with 5132 for a trigger it has written and crashes on
  one made with SQL `CREATE TRIGGER`, so there is no behaviour to match.
- `AdsDDAddProcedure`, `AdsDDAddProcedure100` and `AdsDDRemoveProcedure` add and
  remove a stored procedure in a data dictionary: a StoredProc object under the
  Database holding the input and output parameter lists, the invoke option, and
  either the SQL script (a script procedure) or the external library name and
  function (a library procedure), with its comment in the dictionary memo —
  written as the original engine writes it, byte for byte, cross-checked both
  ways (the records, the memo and the rebuilt dictionary index alike). Removing
  one soft-deletes its object and frees its comment memo. An empty or already-used
  name is refused with `AE_INVALID_OBJECT_NAME` (5132). `AdsDDSetProcedureProperty`
  is not written yet.
- `AdsDDGrantPermission` and `AdsDDRevokePermission` now grant and revoke on every
  object type — views, procedures, functions, packages, links, users, groups and
  the database, as well as tables and their columns — each with its own applicable
  rights, byte-for-byte as the original engine. A view/procedure/function/package/
  link/user/group/database grant works like a table grant (its object rights,
  enciphered, plus the inherit bit for a user); the database is granted by passing
  no object name. Object types that have no permissions are refused.
- `AdsDDGrantPermission` and `AdsDDRevokePermission` grant and revoke on a
  table's columns (object type 4) and with the grant option, not only whole-table
  rights, byte-for-byte as the original engine — cross-checked both ways. A column
  grant keeps each column's own byte in the Permission object's per-column slot and
  aggregates their bits into the table's object-rights; a revoke drops a column's
  bits and lowers the aggregate to what the other columns still hold. The grant
  option writes the with-grant object rights (and, for a column, the with-grant
  per-column rights).
- `system.permissions` is now read, not empty: a row for every grantee (each
  user, then the groups and the built-in groups) and every object of the
  dictionary (its tables and their columns, views, procedures, functions,
  packages, links, users, groups and the database, then the right to create each
  kind of object), each cell the permission level the original engine reports —
  0 none, 1 granted, 2 held implicitly by a built-in administrative group.
  Grants are decrypted with the dictionary key: a table's object rights, and
  either its per-column grants or, without them, the table's rights reflected to
  its columns. Checked row for row against the engine's dumps of three
  dictionaries.
- `system.effectivepermissions` is read too: the same grid as `system.permissions`
  but with each user also holding the rights granted to the groups it belongs to,
  as the original engine rolls them up.
- `AdsDDGetPermissions` returns the permission mask a grantee holds on an object
  (granted or implicit, plus the inherit bit for a user), with `usGetInherited`
  adding a user's groups' grants — byte-identical to the original engine, probed
  and cross-checked both ways (through tflip's own DLL). Permissions on objects
  tflip does not enumerate (publications, subscriptions) are still to come.
- A connection's administrator password (from `AdsConnect60`, or the `Password`
  of a connect string) now recovers the dictionary key, so group memberships and
  other encrypted dictionary properties are read on a password-protected
  dictionary, not only one with the default empty password. Only the
  administrator's own password unlocks the key; any other is ignored, as before.
- `AdsDDGrantPermission` and `AdsDDRevokePermission` grant and revoke rights on a
  table to a user or group, as the original engine does — byte for byte,
  cross-checked both ways. A Permission object under the grantee holds the rights
  mask, enciphered with the dictionary key (the mask and its CRC-32, RC4'd at the
  object's own id); a user's mask carries the inherit bit, a group's does not, and
  `ALL` expands to the table's grantable bits. A second grant to the same grantee
  on the same table accumulates; a revoke clears the bits but keeps the object.
  Granting the first permission raises the dictionary version, as the engine does.
  Only table objects are handled (other object types fail as not implemented),
  column grants and the grant option are not written, and — as for group
  membership — the dictionary key must be recoverable (an empty administrator
  password, every dictionary tflip writes).
- A user's group membership is now read, not only written: `AdsDDGetUserProperty`
  with `ADS_DD_USER_GROUP_MEMBERSHIP` lists the member groups and `DB:Public`
  (`"group1;group2;DB:Public"`, as the engine returns it), and
  `system.usergroupmembers` has a row per membership. The member groups are
  decrypted with the dictionary key, recovered from the connection's
  administrator password (empty for a dictionary tflip writes); without it, a
  user in a real group reads as not implemented.
- Users join and leave groups: `AdsDDCreateUser` with a group, and
  `AdsDDAddUserToGroup` and `AdsDDRemoveUserFromGroup`, write the user's group
  membership as the original engine does — the member groups' object ids, in
  membership order, encrypted with the dictionary key into the user's property
  slot, with the group count in the record. Cross-checked both ways: the
  deterministic rest of the file is byte for byte (the encrypted slot, like the
  login secret, re-salted per dictionary), and the slot tflip writes decrypts to
  the same group ids the engine's does, with the key recovered the same way.
  Adding a user already in a group fails with 5157, removing one that is not a
  member with 5156. A built-in group other than `DB:Public`, and a dictionary
  with a non-empty administrator password (whose key tflip cannot recover), are
  not handled (they fail as not implemented).
- `AdsDDCreateUser` and `AdsDDDeleteUser` create and delete a dictionary user:
  a User object under the Database, with the login secret that authenticates its
  password (its property slot, obfuscated at the user's record as the original
  engine stores it, cross-checked both ways — the secret tflip writes verifies
  the password and rejects a wrong one, decoded as the engine decodes it) and
  its description in the dictionary memo. The deterministic rest of the record
  is byte for byte; the login secret is re-salted on every write, as the engine
  does. Deleting a user frees its description memo.
- `AdsDDCreateUserGroup` and `AdsDDDeleteUserGroup` create and delete a user
  group in a data dictionary: a Group object under the Database, with its
  description stored in the dictionary memo file, written as the original engine
  writes it — byte for byte, cross-checked both ways (the record, the memo and
  the rebuilt dictionary index alike). Deleting a group frees its description
  memo. A reserved, empty or already-used name is refused with
  `AE_INVALID_OBJECT_NAME` (5132), as the engine does; a group with members, and
  the encrypted user subsystem, are not written yet.
- The `AdsDDSet*Property` setters cover many more properties, each written as
  the original engine writes it — byte for byte, cross-checked both ways,
  against engine goldens and through tflip's own DLL:
  - `AdsDDSetTableProperty`: the validation expression and message, the memo
    block size, the trigger types and options, the default index and the
    user-defined property (the table's property blob); the permission level,
    caching mode and transaction-free flag (the record header); the auto-create
    flag (a presence slot); and the wide validation expression (stored as UTF-8
    with a byte-order mark). The flags the engine silently drops here (web-delta,
    concurrency and caching "both") stay reported as not implemented.
  - `AdsDDSetFieldProperty`: the user-defined property, the can-null flag (the
    inverted `NOT_NULL` presence slot), and the wide default, minimum and
    maximum values (converted to the connection's code page, failing with
    `AE_UNICODE_CONVERSION` on a character it lacks, as the engine does).
  - `AdsDDSetViewProperty`: the view's SQL statement (text and stored length).
  - `AdsDDSetDatabaseProperty`: the Internet and login settings, the full-text
    search word lists, the query-log table, the query-via-root option and the
    user-defined property (value slots), and the verify-access-rights,
    encrypt-communication and disable-DLL-caching presence flags.
  The index, field and database properties the engine serves read-only (it
  answers 5126) stay reported as not implemented. The wide view statement
  (`ADS_DD_VIEW_STMT_W`) is not handled yet (its stored length is the wide input
  length, which needs the engine to carry it).

## v0.13

- `AdsDDAddIndexFile` adds an existing index file to a table already in a data
  dictionary: an Index object and a Key object per order, written as the
  original engine writes them (byte for byte, cross-checked both ways).
- `AdsDDFreeTable` frees a table from its data dictionary, clearing the binding
  in the table's own file (its object id and the dictionary back-reference) so
  it becomes a free table again, as the original engine does it — byte for byte,
  cross-checked both ways.
- `AdsDDSetFieldProperty`, `AdsDDSetIndexProperty` and `AdsDDSetDatabaseProperty`
  set properties of a field, an index order and the database: the comment
  (`ADS_DD_COMMENT`), the string-valued field properties (default, minimum,
  maximum and validation message) and the database's customer version numbers,
  written as the original engine writes them — byte for byte, cross-checked both
  ways. A field's blob properties are a read-modify-write that preserves the
  object's other properties. Their remaining properties, and
  `AdsDDSetProcedureProperty`, are not handled yet.
- `AdsDDSetTableProperty` and `AdsDDSetViewProperty` set a table's or view's
  comment (`ADS_DD_COMMENT`): the text goes into the dictionary memo file and
  its reference into the object's record, as the original engine writes them,
  byte for byte. Their other properties are not handled yet.
- `AdsDDRemoveView`, `AdsDDRemoveTable`, `AdsDDRemoveIndexFile` and
  `AdsDDDeleteIndex` remove objects from a data dictionary, and
  `AdsDDRenameObject` renames a view: the records are soft-deleted or rewritten
  in place and the dictionary index rebuilt exactly as the original engine does
  it (byte for byte, cross-checked both ways — the deleted-record free list,
  the record statuses and the rebuilt index all match, and the original engine
  reads the result). `AdsDDRemoveTable` removes the table's fields, index
  objects and keys with it, and with its flag the files too. Renaming a table,
  a field or an index is not done yet (it renames files and rewrites paths).
- `AdsDDAddView` and `AdsDDAddView100` add a view to a data dictionary: a View
  object holding the SQL statement and its length, with the comment (if any)
  stored in the dictionary memo file, written as the original engine writes
  them — byte for byte, cross-checked both ways, the record, the memo and the
  rebuilt dictionary index alike; the original engine opens a view tflip adds.
- `AdsDDAddTable` now binds the added table's own file to the dictionary as the
  original engine does (its object id and the dictionary's path, stamped into
  the `.adt` header), so the original engine opens a table — and a view over
  it — that tflip added, where before it refused them (error 7074).
- `AdsDDAddTable` and `AdsDDAddTable90` add an existing ADT table to a data
  dictionary: a Table object, a Field object per column, and for each of the
  table's index files an Index object with a Key object per order, written as
  the original engine writes them (byte for byte, cross-checked both ways, the
  records and the rebuilt dictionary index alike), and the connection sees the
  new table at once. Non-ADT tables are not added yet.
- `AdsDDCreate` and `AdsDDCreate101` create a data dictionary and connect to
  it, the first dictionary-writing functions. The `.add`, its `.am` memo and
  its `.ai` index (the dictionary's seven system orders) are written as the
  original engine writes them — byte for byte but for the Database object's
  randomly-salted `ADSSYS` login secret, which tflip generates afresh and the
  original engine authenticates. Cross-checked both ways: the original engine
  opens and logs in to a dictionary tflip created, and tflip's files match the
  engine's. An encrypted dictionary (the `usEncrypt` flag) is not made yet.

## v0.12

- Encrypted tables are read, in both schemes the engine uses: the default
  RC4 and AES-128/256. Given the password, an encrypted table's records,
  columns, memo values and index orders all decrypt to the bytes the
  original engine returns, and the table browses in record and key order
  alike. A free table opens with the connection's password; a
  dictionary-bound one with the table password its dictionary releases after
  login, so an encrypted data dictionary is read too. The ciphers are
  reimplemented and verified against the engine's own fixtures (RC4 keyed by
  the password with the engine's per-item starting state; AES as
  PBKDF2-HMAC-SHA1 and AES-CTR with the stored salt and IVs). The functions
  that enable, disable or re-key encryption are still stubs.

## v0.11

- Every remaining function of `ace.h` and of the unpublished interface is
  exported, 465 in all; those tflip does not implement (69 now) fail with
  `AE_NOT_IMPLEMENTED`.
- `AdsApplicationExit` closes every connection and table, writing what is
  pending.
- SQL on Clipper, FoxPro and VFP tables: the `AdsStmtSet*` settings are
  kept, and `SELECT`, `INSERT`, `UPDATE`, `DELETE`, `CREATE TABLE`,
  `CREATE INDEX`, `DROP INDEX`, `DROP TABLE` and `ALTER TABLE` work on the
  statement's table type, as the original engine does them.
- `AdsDeleteIndex` deletes the orders of `.cdx` files.
- Transactions, first part: `AdsBeginTransaction`, `AdsCommitTransaction`,
  `AdsRollbackTransaction(80)`, savepoints and SQL's transaction statements
  keep the count and its errors as the original's remote server does, and
  pack, zap and reindex are refused in one; their writes are not held back
  yet. `tflip.ini` `[transactions] mode = count` (`tflipd --transactions
  count`) gives the original local server's behaviour.
- Transactions hold their writes back, as the original's server does, byte
  for byte: records marked in the files and their new images kept until
  the commit, appends written blank, unique keys a transaction took refused
  to others, commits, rollbacks (to savepoints too, and of a connection that
  closes) applied to tables, memo files and orders.
- Transactions are logged to disk, and those a crash interrupted are
  committed or rolled back when a table is next opened (and by
  `AdsFailedTransactionRecovery`, and `tflipd` when it starts).
  `tflip.ini` `[transactions] log` names the log folder. A transaction's
  end writes the changes its tables have pending into it, as the
  original's server does.
- In a transaction, `AdsAppendRecord` logs and writes the marked blank
  record at once, and a cancelled append stays the transaction's until it
  ends (then deleted); a DBF record without a memo block gets the memo
  values set in a transaction at once; `AdsFlushFileBuffers` leaves the
  pending record alone in a transaction and on `tflipd`, as the original's
  server does.
- Transactions and several processes: another process passes a
  transaction's new keys by, in walks and seeks, as other connections do;
  a table closed in a transaction keeps the transaction's record locks
  (and the file open) until it ends; a table opened while another thread
  recovers a crashed transaction waits for the recovery.
- SQL and transactions, as probed on the original's server (`tx_sql`):
  `SET TRANSACTION AUTOCOMMIT_ON` puts each `INSERT`, `UPDATE` and
  `DELETE` in a transaction of its own, committed after it or rolled back
  if it fails; `AUTOCOMMIT_OFF` begins one a statement leaves open (one
  that fails takes it back); beginning a transaction sets the mode back to
  `EXPLICIT`. An `UPDATE` or `DELETE` passes by records it cannot lock and
  fails with 5035 at the end. A rollback takes records back in the order
  the transaction first wrote them. Another connection going to a record
  a transaction appended is on no record.
- Relations: `AdsSetRelation`, `AdsSetScopedRelation` and
  `AdsClearRelation`, as the original engine has them, locally and through
  `tflipd`.
- Fixed, as probed: `AdsOpenTable` with the option 0x10000 hands out the
  handle 0; `AdsGotoTop`, `AdsGotoBottom` and `AdsReindex` given something
  that is neither a table nor an order fail with `AE_INVALID_HANDLE`;
  `AdsGotoRecord` to a deleted record puts the table on it.
- Fixed in transactions, from a review and a stress test of six threads:
  an append onto a slot a rollback had just freed; memo values of an
  image a savepoint goes back to; a log removed after a commit that failed
  half-way; two commits of one connection at once; a write landing after
  its transaction ended; recovery skipping index rebuilds, or taking a
  running transaction's log on platforms with process-wide locks; two
  relations set at once making a cycle.
- The protocol is at version 32.
- Fixed: a VFP table's header year (years since 1900, not the year mod
  100); NMemo values and AutoInc fields kept by a DBF restructure; columns
  named as the `SELECT` writes them.
- `tflip-ace` reaches the engine only through its API; the engine keeps
  the default path as a session setting.
- The record cipher of encrypted ADT tables is reimplemented clean-room and
  verified against the original engine (RC4 keyed by the password, with the
  engine's own per-record starting state): given the password, an encrypted
  table's records decrypt in `tflip-storage` to the bytes the original
  hands out. Reaching this through the API, and reading encrypted
  dictionaries, is still to come.

## v0.10

- Creating ADT and DBF tables and their orders (`AdsCreateTable`,
  `AdsCreateIndex`, SQL `CREATE TABLE` and `CREATE INDEX` on ADT tables),
  orders built on tables with records, binary orders, `.idx` files and
  custom CDX orders.
- Restructuring (`AdsRestructureTable`, `ALTER TABLE`), packing, zapping,
  reindexing, recalling, copying and converting tables.
- Optimized AOFs, with comparisons looked up in ADI and CDX orders; SQL
  plans and `UPDATE` and `DELETE` that look their records up in indexes.
- `AdsFindConnection(25)`, `AdsGetIntProperty`, `AdsRegisterUDF`,
  `AdsGetActiveLinkInfo`, `AdsFindServers`, `AdsCloneTable` and the
  management functions for locks and threads.
- 387 exports.

## v0.9

SQL plans: `SHOW PLAN FOR`, from the operator tree that runs. `INSERT`,
`UPDATE` and `DELETE`.

## v0.8

Writing Clipper, FoxPro and Visual FoxPro tables, with DBT and FPT memo
values and NTX, CDX and IDX orders kept current. SQL joins, grouping,
`UNION` and subqueries.

## v0.7

Writing ADT tables: records, memo values, ADI orders and locks, byte for
byte as the original engine writes them.

## v0.6

Data dictionaries: users, tables and views by name, objects and their
properties.

## v0.5

DBF tables with NTX, CDX and IDX indexes; collations and code pages.

## v0.4

ARC's table properties, active queries and SQL utility work, locally and
on `tflipd`.

## v0.3

ARC seeks, scopes, filters and runs SQL, locally and on `tflipd` over TLS.

## v0.2

ARC lists a table's ADI indexes and browses it in index order.

## v0.1

ARC opens tflip's `ace32.dll` and shows the data of ADT tables.
