# Online backup and restore (fixtures/backup/)

What the ADS 12 evaluation server (remote server, under Wine, setup in `LOCAL.md`)
answers and writes when `sp_BackupDatabase`, `sp_BackupFreeTables`,
`sp_RestoreDatabase` and `sp_RestoreFreeTables` are called, and what the local
server answers. Recorded with `tools/ace/backup/run.sh <scenario> [local]`
(driver: `tools/ace/events/ev.c`; scripts in `tools/ace/backup/scripts/`,
`#include` and `#table` lines expanded by `run.sh`; the table generator is
`mk_tables.sh`). Analysis helpers: `show.py`, `lstree.py`, `cmptree.py`,
`diffrange.py`, `adtstat.py`, `dbfmemo.py`, `tarinfo.py`, `hexs.py`.

Every scenario is a folder `fixtures/backup/<scenario>/` with

- `result.json`: one object per script line, as `ev.c` writes them: `rc`,
  `error` (the `AdsGetLastError` text), and for a result set `fields`
  (`[name, ADS type, length]`) and `rows` (every column as `AdsGetField` gave
  it; `null` for a NULL column). At most 100 rows are read. Lines that start
  with `shell`, `ttype`, `connopt`, `open`, `lock` are probe commands.
- `errlog.txt`: the rows the server added to its own error log
  (`C:\ads_err.adt` of the server's prefix) during the scenario, as
  `code source:line file | more info`, no times (remote scenarios only).
- `tree/`: every file under the scenario's folder when the script ended,
  byte for byte. `src` (or `fsrc`) is the source (a dictionary `src\d.add`, or
  a folder of free tables), `img*`/`b*`/`ar*` the backup images, `dst*`/`q*`
  the restored folders, `srcA/B/C`, `snap1..4` copies taken between two
  statements.

Git keeps no empty folders. These folders are empty in the original and the
tests must create them: `s41_free_restore/tree/dst7`, `dst8`, `dst10` and
`s23_option_matrix_free/tree/qf6`, `qf15`, `qf17`, `qf18` (sp_RestoreFreeTables
creates its destination folder even when it restores nothing).

The test data: tables `t_adt` (ADT), `t_cdx` (CDX), `t_vfp` (VFP), `t_ntx`
(NTX/Clipper), in the dictionary `src\d.add` (`scripts/dd_all.txt`): fields
`id INTEGER, name CHAR(20), qty NUMERIC(10,2), d DATE, flag LOGICAL, note MEMO`,
ten rows, rows 3 and 6 deleted, row 2's memo grown afterwards (so a dead memo
block exists), orders `<table>_name` and `<table>_d`; a view `v_adt`, a
procedure `p_one`, a user `bkuser` (password `pw`). The free tables
(`scripts/free_all.txt`) are `f_adt.adt`, `f_cdx.dbf`, `f_vfp.dbf`, `f_ntx.dbf`
in `fsrc`, the same contents.

The server also logs to its error log (not the result set) 2/7041/5168 entries
for every table and index a restore creates; `errlog.txt` has them. They are
not part of the contract of the call.

## Scenarios

| Scenario | What it records |
| --- | --- |
| `s01_full` | sp_BackupDatabase of the whole dictionary; image vs source |
| `s02_restore_empty` | sp_RestoreDatabase of that image into an empty folder from a free connection; image vs restored vs source |
| `s03_restore_over` | restore over an existing dictionary (new rows in some tables, a row only in the destination) |
| `s03_dd_objects` | extras: dropped view, unique index, second table, group |
| `s04_open_image` | opening a table of the image directly (7074, 7041, 7008) |
| `s05_restore_pw`, `s05b_restore_pw_unneeded` | SourcePassword wrong, `''`, NULL, right, and a password where none is set |
| `s06_restore_user` | `User=` a DB:Backup member, a plain user, an unknown user, a wrong password |
| `s07_restore_renamed` | restoring to a differently named `.add` |
| `s08_restore_extra` | restoring an image with an extra table, then an image lacking a table the destination has |
| `s09_dontoverwrite`, `s10_nowarnings` | DontOverwrite, NoWarnings on a dictionary restore |
| `s11_include_exclude` | include= and exclude= (names, case, spaces, unknown, with `.adt`, empty) on backup and restore |
| `s12_metaonly` | MetaOnly on backup and restore |
| `s20_backup_errors` | permissions, destinations, option strings, argument counts of sp_BackupDatabase |
| `s21_restore_errors` | the same for sp_RestoreDatabase |
| `s22_option_matrix`, `s23_option_matrix_free` | 21 option strings against each of the four procedures |
| `s30_local_local` | the four procedures on the local server |
| `s40_free_backup` | sp_BackupFreeTables: masks, TableTypeMap, connection table types, include/exclude, bad paths |
| `s41_free_restore` | sp_RestoreFreeTables: overwrite, DontOverwrite, NoWarnings, include/exclude, sources that are missing |
| `s50_diff` | PrepareDiff, three Diff backups with edits, appends and a delete between them (copies of source and image after each step) |
| `s51_diff_noprep` | Diff without PrepareDiff, to a new folder, after a full backup |
| `s52_diff_reinit` | Diff after pack, zap, alter and encrypt |
| `s53_diff_restore`, `s54_diff_dbf`, `s55_free_diff` | restore of a differential image; Diff on DBF tables; Diff on free tables |
| `s56_diff_writes`, `s57_diff_tx`, `s58_diff_reinit_hdr`, `s59_diff_dd`, `s59b_diff_newtable` | the source files between PrepareDiff and Diff (see "What a write leaves in the source" below); snapshots `snap*` are copies of the source's `*.ad?` after each step, `img*` copies of the image |
| `s92_diff_memo_slot`, `s93_diff_memo_passes`, `s94_diff_memo_multi`, `s95_diff_memo_size`, `s96_diff_memo_more`, `s97_diff_memo_first` | where a Diff puts a changed record's memo value (slot, several records per pass, several Diffs, sizes, large values, two memo fields, appends, deletes; see "Where a Diff puts..."); `snap0` is the PrepareDiff image, `snapN` the image after Diff N |
| `s60_dd_encrypted`, `s61_free_encrypted` | RC4 tables in a dictionary and as free tables: passwords for backup and restore |
| `s70_archive`, `s71_archive_long`, `s72_archive_free` | ArchiveFile / ArchiveFileCompressed, names over 99 characters |
| `s80_open_shared`, `s81_uncommitted_tx`, `s82_exclusive`, `s83_exclusive_free`, `s84_exclusive_restore`, `s85_concurrent` | tables open, locked, in a pending transaction, exclusive; three backups at once (`s85` keeps only `result.json`) |
| `s90_aes`, `s91_aes_free` | AES: the evaluation server cannot (5214 / 7160) |

## The result set

Seven columns, as the cursor types them (`AdsGetFieldType`, `AdsGetFieldLength`):

| # | Name | ADS type | Length |
| --- | --- | --- | --- |
| 1 | `Severity` | 11 (Integer) | 4 |
| 2 | `Error Code` | 11 | 4 |
| 3 | `Error Message` | 5 (Memo) | 9 |
| 4 | `Table Name` | 5 | 9 |
| 5 | `Additional Info` | 5 | 9 |
| 6 | `Source File` | 4 (Char) | 32 |
| 7 | `Source Line` | 11 | 4 |

An empty set means success (the cursor exists with the seven fields and no
rows). Source File is always `..\ACE\aceunpub.cpp` padded with spaces to 32.
Rows are in processing order, tables by name (`t_alter, t_enc, t_pack, t_zap`).
NULL columns are marked below. "dest" is the destination path as given,
"src" the source path as given (a trailing `\` is added by the server where
shown). All rows seen:

Backup, differential (severity 5, code 0, message NULL, table `dest\d.add` for
the dictionary file, `dest\<table>` without extension for a dictionary table,
`dest\<file>` with extension for a free table):

| Line | Additional Info |
| --- | --- |
| 6660 | `Error opening backup image of table for differential backup. Assuming this is a new table and creating a new table in the backup image automatically.` (Diff into a folder without an image; no PrepareDiff before) |
| 6693 | `Table structure does not match the backup image table structure. Re-initializing the differential backup for this table.` (after ALTER TABLE) |
| 6709 | `The differential backup times do not match. Re-initializing the differential backup for this table.` (after pack, zap, encrypt; and when the image folder holds a full backup made later than the PrepareDiff) |

Backup, other:

| Sev | Code | Message | Table Name | Additional Info | Line | When |
| --- | --- | --- | --- | --- | --- | --- |
| 7 | 5132 | `Error 5132:  The specified database object name is not valid.  "nosuch" is not a table or view in the data dictionary.` | `nosuch` (the name as given, with a leading space if the list had one) | `Error opening for backup.` | 9680 | include= has an unknown name besides valid ones |
| 7 | 7008 | `Error 7008:  The specified table, memo file, or index file was unable to be opened.` | `t_adt` | `Error opening for backup.` | 9680 | a dictionary table another connection holds open exclusively (the other tables are backed up) |
| 7 | 7008, 7016, 5025, 7041 | `Error <code>:  ...  <full path of the file>` (7016: `Corrupt table.  Make sure you are not attempting to open a DBF with an ADT table type or vice versa.`; 5025: `An invalid table type was specified.  Valid choices are ADS_ADT, ADS_VFP, ADS_CDX, and ADS_NTX.  The table type must support the memo file type to be opened, if applicable.  Should have been ADS_NTX.`; 7041 `File not found.  Verify the specified path and file name is correct.  ...`) | full path of the file | `Error opening for backup.` | 9695 | free tables: a file of the mask opened with a wrong type, an exclusive table, an unknown include name |
| 7 | 0 | NULL | `src\` (with the trailing backslash) | `No source tables found. Verify source path is valid.` | 9663 | no file matches the mask; also MetaOnly on free tables; a missing source folder |
| 10 | 7131 | NULL | the table name (`t_big`, `f_cdx.dbf`) | `Error initializing table for hot backup. See error code for more details.` | 8009 | the table is being backed up by another call |
| 7 | 7131 | NULL | the dictionary file's UNC path (`\\127.0.0.1:6262\data\...\src\d.add`) | the same | 8048 | the dictionary file is being backed up by another call |
| 5 | 5097 | `Error 5097:  The password given is not valid.  Encrypted table encountered but no passwords were supplied.` | `dest\f_e1.adt` | NULL | 6842 | free table backup, encrypted table, no password at all |
| 5 | 5097 | `...  Encrypted table encountered but no password was passed in for this table.` | `dest\f_e2.adt` | NULL | 6854 | a list of `table=password` pairs without this table (names with extension; `f_e1=pass` does not match) |
| 7 | 5097 | NULL | `f_e1.adt` | `Error backing up table data.` | 9817 | follows each of the two rows above |
| 7 | 7060 | `Error 7060:  Invalid encryption password.  The specified password does not match the password being used in the table.` | `f_e2.adt` | `Error backing up table data.` | 9817 | wrong password; a single password is used for every encrypted table |
| 10 | 3 | NULL | `dest\` | `Error creating archive file.` | 8927 | `ArchiveFile=` with an empty name (followed by two more rows) |
| 10 | 7040 | NULL | `dest\` | `Error creating archive file.` | 8930 | ditto |
| 10 | 7040 | NULL | NULL | `Error creating backup archive.` | 9840 | ditto, and after the next row |
| 10 | 5050 | NULL | `dest\<file name of 100 characters>.adt` | `The file or path name is too long.  It exceeds the archive header limit.` | 8558 | a file name over 99 characters; then the row with line 9840 and table name `x.tar`; the call succeeds and the individual files stay |

Restore:

| Sev | Code | Message | Table Name | Additional Info | Line | When |
| --- | --- | --- | --- | --- | --- | --- |
| 10 | 32 | NULL | `src\d.add` | `Unable to restore the dictionary metadata. Restore operation abandoned. Verify the target dictionary is not currently opened by another user, or by the current connection. If you are trying to restore the dictionary you are currently connected to, use a free connection to call sp_RestoreDatabase, rather than connecting to the target dictionary and then calling sp_RestoreDatabase.` | 10655 | the destination dictionary is open (also restoring into the image's own folder); code 3 when the destination path cannot be made (`Z:\usr\...`). Nothing is restored |
| 10 | 7008 | `Error 7008:  The specified table, memo file, or index file was unable to be opened.` | `t_adt` (one row per table) | `Unable to open/create table.` | 11046 | restoring `img\d.add` to `img\other.add` (tables of the image folder in use) |
| 10 | 7041 | `Error 7041:  File not found.  Verify the specified path and file name is correct.  src\t_cdx.dbf` | `src\t_cdx` | `Unable to open source table.` | 10850 | include= names a table the image lacks (a name not in the dictionary reads `src\ nosuch.adt` with the name as given); tables the image lacks are skipped silently without include= |
| 7 | 0 | NULL | `src\` | `No source tables found. Verify source path is valid.` | 9990 | free restore: no tables in the folder; MetaOnly |
| 10 | 80 | NULL | `src\f_adt.adt` | `Error copying free table during restore operation.` | 10033 | free restore with DontOverwrite, the file exists; code 32 when the destination is the image folder; code 2 when include= names a missing table |
| 10 | 5097 | `Error 5097:  The password given is not valid.  Encrypted table encountered but no passwords were supplied.` | `dst\f_e1.adt` | `Unable to reindex table after it was restored.` | 10157 | encrypted free table, no password; the table is copied |
| 10 | 7060 | NULL | `dst\f_e1.adt` | `Unable to reindex table after it was restored. Error setting encryption password.` | 10183 | wrong password |

The file name in a restore row's "Table Name" is the **image's** path (`src`),
not the destination's.

## Errors the call itself raises (rc 7200, the text is `Error 7200:  AQE Error:  State = HY000;   NativeError = <n>;  [SAP][Advantage SQL Engine][ASA] ...`)

| Native | Text after `[ASA]` | When |
| --- | --- | --- |
| 5054 | `Error 5054:  The command cannot be completed with the current user permissions.  Cannot call BackupDatabase on a free connection.` | sp_BackupDatabase on a free connection |
| 5054 | `...  Must be administrator or member of DB:Backup to use this functionality.` | sp_BackupDatabase and sp_RestoreDatabase on a dictionary connection of a user in neither DB:Backup nor DB:Admin (members of DB:Backup, DB:Admin and ADSSYS succeed) |
| 5054 | `The command cannot be completed with the current user permissions.` (no extra text) | `User=` names a plain user |
| 5100 | `The given user name is not valid. It may be too long or of length 0.` | sp_RestoreDatabase: SourcePassword wrong, empty or NULL for the ADSSYS of a dictionary that has a password; any non-empty password for ADSSYS where none is set; `User=` of an unknown user |
| 7078 | `The Advantage Database Server cannot authenticate the user.  Make sure the user name and password are correct.` | `User=bkuser` with a wrong password |
| 7040 | `File creation error.  Verify the specified path and file name is correct and that no other users have the file open.` | sp_BackupDatabase: the destination is a file, the source folder itself (its `d.add` exists), a folder the server cannot write (`Z:\usr\...`) |
| 5020 | `Error 5020:  The path given was not valid.  invalid destination path` | a relative destination (`relimg`), a UNC `\\127.0.0.1\c$\...` |
| 5020 | `...  Source path must include exisiting dictionary file name.` (sic) | sp_RestoreDatabase: source is a folder, a table, a missing path |
| 5020 | `...  Destination path must include existing dictionary file name.` (sic) | sp_RestoreDatabase: destination without `.add` name |
| 5021 | `Error 5021:  The pointer given was not valid.  Parameter 5` | sp_BackupDatabase with an empty or NULL destination (parameter 5 counts the ACE call's arguments); sp_RestoreDatabase with an empty or NULL source (`Parameter 3`) or destination (`Parameter 5`) |
| 5021 | `The pointer given was not valid.` | sp_BackupFreeTables with an empty or NULL SourceMask |
| 5019 | `Error 5019:  An invalid option was specified.  <text>` | `Unknown backup/restore option encountered: Bogus` (`=`, `;` are reported the same way, the text is the rest of the string from the bad token: `;exclude=t_cdx` for `include=t_adt;;exclude=t_cdx`); `missing = in table inclusion/exclusion list` (`include`); `missing = after the User option`, `...ArchiveFile option`, `...ArchiveFileCompressed option`; `missing = in table type map`; `Cannot specify PrepareDiff and Diff for the same backup operation.`; `DONTOVERWRITE option is only supported on restore operations.` |
| 5132 | `The specified database object name is not valid.` | include= whose names are all unknown (backup and restore); an include of the dictionary's view; `' include = t_adt ; '` (the key is trimmed, the name is not: ` t_adt ` is unknown) |
| 5198 | `Error 5198:  The backup operation failed. See additional error information for more details.  The checksum for an archive entry is not correct. The archive file is not valid.` | restore with `ArchiveFile=` naming a gzip file |
| 7008 | `...  Error opening archive file <dir>\nofile.tar.` | restore with an archive that is missing |
| 7160 | `Unable to find or load one of the OpenSSL libraries.` | `DDPassword=` on restore (the server has no OpenSSL) |
| 2129 | `[SAP][Advantage SQL Engine]Unequal number of insert columns and insert values or the incorrect number of stored procedure input parameters` (State S0000) | too few or too many arguments |

All four procedures on the **local server** (any arguments, valid or not, free
or dictionary connection): rc 7200, `NativeError = 5042;  ... Error 5042:  The
requested feature has not yet been implemented.  Backup and restore
functionality is not supported on Advantage Local Server connections.`
(`s30_local_local`).

## Options (all four procedures unless said)

Keywords are case-insensitive (`nowarnings`, `DONTOVERWRITE`, `tabletypemap`);
the option string is split at `;`; the key is trimmed and the value is not.
Accepted and ignored on every procedure: `User`, `DDPassword`, `NoWarnings`,
`ForceArchiveExtract`, `TableTypeMap`, `PrepareDiff`, `Diff` (also on restore,
where nothing changes), a repeated `include=`. `DontOverwrite` on a backup is
5019. `MetaOnly` on a dictionary backup writes only `d.add` and `d.am` (on a
dictionary restore only the dictionary; the tables of the image are not
restored); on free tables it is the "No source tables found" row. `DDPassword`
on a dictionary restore needs OpenSSL (7160); `User=` on a dictionary restore
is the user the restore connects as (the `SourcePassword` is that user's).

`include=` / `exclude=` on a dictionary: table names of the dictionary,
case-insensitive; the image's files are named with the **case of the include
name** (`include=T_CDX` writes `T_CDX.dbf`, `T_CDX.fpt`); an `.adt` after the
name is accepted; names are not trimmed (`include=t_adt, t_ntx` has the unknown
name ` t_ntx`); `include=` with nothing writes the dictionary only; exclude wins
over include; an unknown name in exclude is ignored; views are not tables.
On free tables the names are file names; **include does not narrow the mask**:
`include=f_adt.adt,f_cdx.dbf` with `*.adt;*.dbf` processed the other tables of
the mask as well and tried `f_cdx.dbf` a second time (7131 row), an include
name outside the mask is added and opened with the mask's type (7016), an
unknown name gives the 7041 row; `include=f_adt` (no extension) with `*.adt`
changed nothing. exclude narrows (`s40_free_backup`, `b_exc`). Restore of free
tables: include= with a missing file is code 2, narrows to the named files.

## sp_BackupDatabase, full: the image

Files in the image (`s01_full/tree/img`): `d.add`, `d.am`, and per table the
`.adt` + `.adm` (ADT) or the `.dbf` + `.fpt`/`.dbt`; **no** `.adi`, `.cdx`,
`.ntx`, `d.ai`. The image of a dictionary that was never restored has no
`d.ai`; a restore (or any connection) creates it. Dictionary files `d.add` and
`d.am` are byte-identical to the source. Backup creates the destination folder
and its parents. An existing image is overwritten file by file (other files stay).
Table files of a table that is not backed up (include/exclude) are not touched.
The `.add` of a failed restore is left in the destination folder together with
`d.am` and `d.ai` (`s05_restore_pw/tree/dwrong`).

The image of a table is a copy that is made in the table's own type: the records are copied in
order, memo values are written afresh, and the header is written as of a free table.

**ADT** (`t_adt.adt`, `s01_full`): header differences from the source:

| Offset | Source | Image |
| --- | --- | --- |
| 0x14 u32 | 2 (the table has a structural index) | 1 |
| 0x18 u32 | record slots 10 | 8: **deleted records are dropped**, the others keep their order and are renumbered (the index entries of the source are therefore not valid for the image) |
| 0x28 u32, 0x30 u32 | free-list head 6, count 2 | 0, 0 |
| 0x5C | `.\d.add\0` (the dictionary path) | zeros |
| 0x160 u32 | 2 | 0 |
| 0x58 u32 flags | 0x11 | 0x11 (0x15 for an encrypted table) |
| 0x174 u32 | 0 | 0 (a differential image keeps the stamp, below) |
| records | status 0x04 / 0x05 | all 0x04 (0x06 for an encrypted table); the data bytes verbatim |

The rest of the header (field descriptors, password check at 0x38.., encryption
type 0x4C, `0x50` autoinc) is the same. **An encrypted table's image has the
field descriptor area (0x190..0x640) in the clear** (`id\0...`) while the
record data stays enciphered with the table's key; restore enciphers the
descriptors again and the restored file equals the source byte for byte
(`s60_dd_encrypted`). The free-table image of an ADT table
(sp_BackupFreeTables) differs from the source in 0x18/0x28/0x30 and the
records the same way, but 0x14, 0x5C (not set), 0x160 and the `.adi` stay.

**ADT memo** (`t_adt.adm`): same size (64 KB, 256-byte blocks from 0x8100 here);
the memo values of the surviving records are written again from the first block in
record order (a dead block of the source and the memos of deleted records are
gone); the u32 at 0x8008 changes (0x3dd to 0xff); the rest of the file is zero.

**DBF family** (`t_cdx`, `t_vfp`, `t_ntx`): the `.dbf` keeps **all records,
deleted ones too** (the delete flag byte), the header date and counts; byte 0x1C
loses its structural-index bit (`01` to `00` for CDX, `03` to `02` for VFP, NTX
has `00` already); the memo pointers of the records (10 ASCII digits in CDX/NTX,
the last 4 bytes little-endian in VFP) are renumbered: the memo file is written again, one
value after the other in record order, a value taking `ceil((8 + length) / 64)`
blocks in an `.fpt` (block size 64, first block 8), a block in a `.dbt` (512);
the header of the `.fpt` has the new next-block number (32 to 30); deleted
records keep their memo values. For the `.dbt` of the NTX table the only
change is one byte, 0x1A at 0x6CF. No `.cdx`/`.ntx` is written.

**Free tables** (`s40_free_backup`): the table is converted to the type it is
opened with: TableTypeMap, else the connection's `TableType` (a free
connection's default is ADT, so `*.dbf` without either fails with the 7016 rows).
A VFP table opened as CDX is written as a CDX table (`0xF5`, 767 bytes, `.fpt`
with 64-byte blocks); opened as NTX it is written as `0x83` with a `.dbt`
(512-byte blocks); a CDX table opened as VFP becomes VFP (`0x30`, 969 bytes);
an NTX table opened as CDX or the reverse fails with 5025. **Structural indexes
are copied verbatim** into the image (`f_adt.adi`, `f_cdx.cdx`: the same
bytes; for ADT that is stale, the records were renumbered). Non-structural
indexes (NTX) are not backed up. Memo files are written as above. An extension in
the mask other than the table types (`*.*`) makes the index and memo files rows
with 7016/7008. Masks are case-insensitive, `;`-separated; a mask without a
wildcard names one file. Nothing is written for a table whose open failed.

## sp_RestoreDatabase

From the image (`s02_restore_empty`): it copies `d.add`, `d.am` (the restore
creates `d.ai`), restores every table in the dictionary's order, **creating
`.adi`, `.cdx` and `.ntx` files** (every index of the dictionary, including
NTX files `t_ntx_name.ntx`; the rebuilt CDX/NTX files are byte-identical to
the source's, the `.adi` differs from the source's only in the record numbers
of its entries). The tables:

- ADT: header 0x14 1 to 2, 0x5C gets `.\d.add`, 0x160 0 to 2; the records
  and memo file are the image's (the restore does not change slots);
- DBF family: byte 0x1C gets its bit back; memo and records the image's.

Restoring a table of the image that is a differential image packs it: the
ten-slot image with one deleted record gave a nine-slot table (`s50_diff`, `dst`).
A restore **replaces the destination dictionary** `d.add`/`d.am` completely,
whatever it listed; the files of tables it no longer lists stay and are then
`5159 ... The table is a database table. It cannot be opened as a free table.`
(`s08_restore_extra`). The destination dictionary and its tables must not be open
(code 32 row). The call can be made on a free connection or a dictionary
connection. A table of the dictionary that is missing in the image is skipped
without a row. **DontOverwrite does nothing on a dictionary restore**
(`s09_dontoverwrite`: existing tables were overwritten, no rows); on free
tables it gives the code 80 rows and keeps the file. NoWarnings only
suppresses the error-log lines. Restoring from a differential image: as from a full one.

Restoring an image whose tables were opened directly: `7074` for a table
named in the dictionary of the image (`Error 7074:  The table ID stored in the
Advantage Data Dictionary file does no match the ID of the table being
opened.`), 7041 for a table the image has no file for.

## sp_RestoreFreeTables

A file copy (`s41_free_restore`): every file of the image folder is copied to
the destination (`.adt`, `.adm`, `.adi`, `.dbf`, memo, `.cdx`), the destination
folder is created, existing files are overwritten (no row) unless
DontOverwrite; **structural ADT indexes are rebuilt** (the `.adi` differs from
the image's and equals a rebuild, as above); `.cdx` files are copied as they
are (they were valid). TableTypeMap makes no difference to the result. An
encrypted table needs its password for the rebuild (the 10157 / 10183 rows;
the table file itself is copied either way).

## Differential (ADT only)

`s50_diff`: `PrepareDiff` writes a full image like the plain backup **but
without dropping deleted records** (`t_adt.adt` of the image has ten slots,
status bytes normalised to 0x04/0x05), and changes the **source**:

- ADT record status bytes: bit `0x04` ("changed since the last backup") is
  cleared in every record (`04`/`05` become `00`/`01`; an encrypted table's
  `06`/`07` become `02`/`03`); a write sets it again;
- header 0x58: bit `0x10` cleared (0x11 to 0x01; the dictionary's own
  `d.add` 0x13 to 0x03);
- header 0x174 u32 becomes the **backup stamp**, a Unix time in seconds (here
  `0x6aede112`), and rises by one for each Diff (`..113`, `..114`); the image
  header has the same stamp. This changes `d.add` too (it is a table of the
  dictionary).

No file is added to the source folder. `Diff` into the same folder copies only
the records with the changed bit (by slot number; appends use the free slots
first, here slots 3 and 6; the image keeps ten slots) and the memo blocks they
need, then clears the bits in the source and raises the stamp; the image
header gets the same stamp. The image's record status stays 0x04/0x05. The
DBF tables are copied in full again each time (no rows, `s54_diff_dbf`). The
re-initialisation rows (above) are made for a table when the image of it is
missing (6660), its field structure differs (6693) or the stamps differ (6709:
pack, zap, encrypt, and a stale image); the table is then written again in
full. Rows are ordered by table name. `PrepareDiff` twice, `PrepareDiff`
followed by `Diff` at once and `Diff` at once to an image made by `Diff` give
no rows. A `Diff` to a folder that holds a full image (`s51`, `img3`) gives
the 6709 row.

On **free tables** (`s55_free_diff`, `s23_option_matrix_free` bf7, bf8, bf19,
bf20) a `Diff` is the same for an ADT table, the rows say `dest\f_adt.adt`
(with the extension) in `Table Name`; the image's `.adi` is copied by
`PrepareDiff` and by a `Diff` that writes the table whole (bf8, 6660), but **a
`Diff` that only copies changed records leaves it as it was**.

### Where a Diff puts a changed record's memo value (`s92`..`s97`)

(Tables `x_*`: eight records with a memo value each, ADT, a record's `name`
edited unless said; `memoref.py` prints the block each record's memo points at,
for the image of each `snapN`.) The image's memo file is
an ordinary ADT memo file with slabs (see `adm.rs`: a value of 1 to 32 blocks
takes a chunk of a slab of its size), and the `Diff` writes a changed record's
value like this, record by record in slot order:

1. The record's old value is freed at once, then the new value takes the
   lowest free chunk of its size. A record whose value did not change
   therefore stays where it is unless a chunk below its own is free (earlier
   in the pass or from an earlier Diff): it moves there (`s93`, `x_s1` passes 5, 6,
   `x_s4`; `s94` `x_m6` pass 3 moves slots 5, 6, 7, 8 each into the chunk the one before left;
   `s96` `x_y1`: a value of 2 blocks that shrinks to 1 gives its chunk to the
   next record that grows to 2 blocks, in the same pass; `s97` `x_w4`: a record deleted in the
   pass frees its chunk at once for the next record). A value with another number of blocks takes a
   chunk of the other size's slab (`s95`: 1 block goes to a new slab at block 385, 3 blocks too);
   values over 32 blocks work the same with runs (`s96` `x_y2`).
2. **The record in slot 1 is the exception**: its old value is freed only
   after the whole pass, so it always gets a chunk of its own (the lowest free
   one, else the next one of the slab), also above its own (`s92` all `x_a1`,
   `x_b*` and `x_c1`; `s93` `x_s2` pass 7: 130 to 137, free 137 present). A slot above 1 with no free chunk
   below its own stays (`x_a2` .. `x_a8`, `x_b4`..., where the first slab chunk (block 129) is in slot 4
   and does not move). It is the slot, not the chunk: `x_w2` of `s97`
   has slot 1 at block 136 and it moves again. The cause of slot 1's exception is not known.
   (The earlier fit "equal block counts and slot above 2" was this rule seen
   in small tables, where the freed chunks happened to be above the record's.)
3. A deleted record's value goes back at once (`s50`); the old value of a
   record whose new value is empty is freed like any other.

### What a write leaves in the source (`s56`..`s59b`)

Header offsets are of the ADT file (`d.add` too); "stat" is the first byte of
a record. All tables here are ADT with a memo and a BLOB field, five records;
the table names start with `x_` because `ev.c` reads `\t` in a path.

**Writes after PrepareDiff** (`s56`, one table per kind of write):

- The **first write of any kind sets header 0x58 bit 0x10 again** (0x01 to
  0x11); a second write changes nothing more. A table nobody wrote stays 0x01.
  **0x174 is never touched by a write** (stays the PrepareDiff stamp), nor
  is the dictionary: a table write leaves `d.add` byte-identical.
- Field edit (SQL UPDATE): stat 00 to 04, only that record (`x_edit`: 04 00 00 04 00
  after two rows were edited, one of them twice). Append: the new slot has stat 04, header
  0x18 slots 5 to 6 (the file grows by one record, 1385 to 1422 bytes). Delete:
  00 to 05 (0x01 deleted, 0x04 changed) and the free list is set as always
  (0x28 head 3, 0x30 count 1). Recall (AdsRecallRecord, table opened exclusive
  with `AdsShowDeleted(TRUE)`; `goto` on the deleted record answers 5022 but the
  recall still lands): 01 to 04, header flag 0x11; **0x28/0x30 keep the free-list
  entry** (head 2 count 1 after the recall).
- Memo value change: stat 04 on the record, the `.adm` gets the new block and
  its header changes (`s56` `x_memo.adm`: bytes 0x2..0x15 header, 0x50c, 0x804, 0x8002..
  differ); a partial BLOB write (AdsSetBinary, total 20, offset 0 then 10): stat
  04, the record holds the blob pointer/length, the second write changes only
  the memo file bytes 0x860a..0x8614 and nothing in the `.adt`.
- A Diff afterwards (`snap_img`): the image of every table gets the new stamp
  in 0x174 (also the untouched `x_ctl`, whose statuses stay 04 from
  PrepareDiff's image), all statuses 04 (05 for a deleted record that stays
  deleted: `x_del`), flag 0x11; the source has stat 00/01 and flag 0x01.
  **The stamp is a Unix time in seconds, not a counter**: PrepareDiff 0x6aeded5a, a Diff
  three seconds later 0x6aeded5d (`s56`); two Diffs in the same second give the same
  stamp (`s59`, `snap_diff`, `snap_diff2`: both ..21). (The README text above, "rises by one",
  held only because the probes were fast.) No row in the result: a plain edit
  never gives 6709/6693.

**Transactions** (`s57`, a second connection):

- While pending, no header flag changes (0x58 stays 0x01, stamp unchanged), and the
  records show: a record updated or deleted in the transaction has **stat 0x80**
  (`80 80 00 00 00`), a record appended in it **stat 0x11**, in a new slot (slots 5 to 6).
- **Commit**: updated 0x80 becomes 04, deleted 0x80 becomes 05, the append 0x11
  becomes 04; header flag becomes 0x11; the free list is set for the delete
  (head 2 count 1). **Rollback**: updated and deleted records go back to 00
  (no 0x04; header 0x58 stays 0x01), and the **appended record stays in the file as
  stat 05** (deleted + changed, slot 6, free-list head 6 count 1, 0x18 = 6).
  Two Diffs later (`img_during`, `img_after`) the source still has that 05 (`x_rb` `00 00 00 00 00 05`), the
  image still five slots: a Diff neither copies nor clears a record whose stat is 05 from a rollback, and the
  stamp of the table is raised as for the others.
- A Diff while the transaction is pending (`img_during`): it takes the committed
  state; the table's image holds the old five slots (`x_pend` image 5 slots, stats 04),
  the pending records keep their 0x80/0x11 stats in the source, the stamp of
  the source and of every image header is raised for all tables (0x6aeded90), and no
  row is added. After the commit, the next Diff has them (`img_after`: `x_pend` 6 slots,
  `04 05 04 04 04 04`, 04 cleared in the source, record 2 left 01).

**Operations that rewrite the table** (`s58`, each on its own table; PrepareDiff
stamp 0x6aeded99 in `snap0`):

| Operation | 0x58 | 0x174 | stats | Diff row |
| --- | --- | --- | --- | --- |
| sp_PackTable (one edit 04, one delete 05 before) | 0x11 | **0** | records that survive keep their stat (`04 00 00 00`: the edited one still 04) | 6709 |
| sp_ZapTable | **0x01** (not set) | **0** | no records | 6709 |
| ALTER TABLE ADD COLUMN | 0x11 | **0** | **all 04** | 6693 |
| sp_EncryptTable | 0x1d (0x10, 0x08, 0x04, 0x01) | **0** | all 06 | 6709 |
| AdsReindex (ADT, structural index; one record edited before) | 0x11 (from the edit) | unchanged | unchanged (`00 00 04 00 00`) | none |

The stamp 0 against the image's stamp is what gives 6709; the structure
mismatch (6693) is found first. The Diff rows are in table-name order
(`x_alter` 6693, `x_enc`, `x_pack`, `x_zap` 6709). In the Diff that follows
the table is written to the image in full (`x_pack` 4 slots, `x_zap` 0 slots, the encrypted
table with 0x15 and stat 06), the image header stamp is that of the Diff (0x6aeded9d) and
the source gets it, stats cleared (`x_enc` source 02, flag 0x0d), flag 0x01. A second Diff
gives no rows. sp_Reindex is not a procedure (7200; the API call `AdsReindex` is used).

**The dictionary** (`s59`): `d.add` is handled as a table. A new object (view, user)
is appended with **stat 04** and header 0x58 becomes 0x13 (0x03 to 0x13), the slots
grow (16 to 17 to 18); a changed property (sp_ModifyUserProperty USER_PASSWORD,
sp_ModifyTableProperty COMMENT) sets stat 04 on the **row of that object** (slots 0
and 1 here), not an append. A Diff then copies the changed rows (the image's `d.add`
is the full dictionary with all stats 04, size 11632 as the source), clears the bits and
the flag to 0x03, and gives the stamp to source and image (0x6aedee21). DROP VIEW:
stat 01, free list head 17 count 1 in the source; a Diff makes it 05 in the image. A change
to a table's data does not touch `d.add`.

**A table created after PrepareDiff** (`s59b`): the table file is made with 0x58 =
0x11, stamp 0 and stat 04 in every record, and the image has no file for it. The Diff
gives one row, **6660** (severity 5, `...\img\x_new`), writes the table in full to the
image with the Diff's stamp and gives the source that stamp with the bits cleared (flag
0x01). The server's own error log gets three more rows for the missing image
file (code 2 `resrc.c:1919`, and 7041 `dbf.c:20452` and `dbf.c:4361`, all with the
image file's path; see `errlog.txt`). A second Diff (an edit in `x_new` between) has no row.

Not probed: `AdsEncryptTable` through the API apart from `sp_EncryptTable`
(same call); a partial write on a free table; FoxPro/VFP tables (they are copied in full
anyway).

## Archives

`ArchiveFile=name` (a plain tar) and `ArchiveFileCompressed=name` (tar in gzip) are
files inside the destination folder, the name as given; `ArchiveFile=sub\x.tar`
creates the folder `sub` in the destination (`ar6`); an empty name gives the
three rows with 8927, 8930 and 9840 (the call succeeds and the individual files
stay). When both options are
given the compressed one is made. After a good archive the individual files
are gone, the folder has only the archive. A restore takes the archive from
the **image folder** (the `SourcePath` still names `d.add`), unpacks into a
temporary place and removes it, and `ForceArchiveExtract` made no visible
difference in the one probe made (restoring twice into the same folder with and
without it: the tables were overwritten both times). `ArchiveFileCompressed=x.tar`
reads a plain tar as well; `ArchiveFile=x.tar.gz` on a gzip file is 5198.
`include=` applies to a restore from an archive.

tar (`s70_archive/tree/ar1/x.tar`, `tarinfo.py`): ustar, members in the
image's order (`d.add`, `d.am`, then per table in dictionary order the
table file, the memo file), type `0`, name without folder, mode `0000666`
(seven digits and a NUL), uid, gid, uname, gname, devmajor, devminor, prefix
all zero bytes, magic `ustar\0` version `00`, a standard checksum, **one mtime
for every member** (the time of the backup, octal, here `15273360750`; varies
per run), size in octal, each member padded to 512 bytes, then two zero
blocks (1024 bytes) and nothing more. A file name over 99 characters is 5050
and no archive is made.

gzip (`ar2/x.tar.gz`): header `1f 8b 08 00 00000000 04 0b` (deflate, no flags, mtime 0,
XFL 4, OS 11), the deflate stream of the tar is **byte-identical to zlib
level 1, windowBits -15, memLevel 8** (reproduced with Python's zlib), then
CRC-32 and ISIZE. The tar of the free-table archive holds `f_adt.adt`,
`f_adt.adm`, `f_adt.adi`.

## Table in use

(`s80`..`s85`) In these probes a backup was never delayed; it takes the committed state: a pending
transaction's inserts, updates and deletes are not in the image (also its
memo values), a backup after the commit has them, a rolled-back one never
(`s81_uncommitted_tx`, searched in the files). Open (shared) tables, a
locked record and a cursor on a table change nothing and give no rows. A
table another connection holds **exclusively** is the 7008 row (sev 7, line
9680 for a dictionary table, 9695 with the path for a free table) and the
other tables are backed up. Two backups running at the same time: the second
gets the 7131 rows (`s85_concurrent`: two of three calls; **timing
dependent**). A restore into a dictionary that a connection has open is the
code 32 row; an exclusive table in a dictionary the restore does not touch
is no problem (`s84_exclusive_restore`).

## Not probed, flaky, varying

- AES dictionaries and tables: the evaluation server cannot load OpenSSL
  (5214 on `sp_SetDDEncryptionType`, 7160 on connecting to such a
  dictionary and on `DDPassword=`), so there is no recording of an AES backup.
- Times: the tar mtime and the `0x174` stamp differ per run; `ms` in every
  `result.json` too. The paths in the `cmd` of `shell` lines are those of the
  machine that recorded.
- `s85_concurrent` depends on timing; only its `result.json` is kept.
- The first `s22_option_matrix` recording (free backups after many dictionary
  backups in one script) had every free-table backup fail with a 7008 row for a
  table nobody had open; split into `s22`/`s23` it did not repeat. Unexplained.
- A trigger on a table of the dictionary whose body inserted into its own
  `__new` made the server stop (not a backup question).
- The `Table Name` of rows for free tables is the source path as given, so
  tests of those rows must write the same path.
