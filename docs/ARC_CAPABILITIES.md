# ARC32 Capabilities

## Overview

ARC32 (Advantage Data Architect, shipped with ADS 12.0) is a Delphi MDI desktop tool that covers the whole ADS surface: connections, data dictionaries, table design and browsing, SQL with a debugger, every dictionary object type, replication, backup and server monitoring. Everything here comes from the vendor-shipped source in `Program Files (x86)/Advantage 12.0/ARC/Source` (312 files, about 12 MB).

| Aspect | Detail |
| --- | --- |
| Project | `arc32.dpr` / `arc32.dproj`, plus design-time package `adstools.dpk` (`arctoolsd*.dpk` for Delphi 5, 6, 7, 2009) |
| Data access | Advantage TDataSet components (`AdsTable`, `AdsData`, `AdsFunc`, `ace`); the code calls about 150 distinct `Ads*` API names directly |
| Third-party UI | DevExpress (grids, bars, tree lists), InfoPower, kbmMemTable, ReportBuilder (printing), OpenQBuilder (`OQBE*`, `qb*` units) for the visual query builder |
| Shell | Main MDI window (`main.pas`) with a docked Connection Repository tree (`ddmainscreen.pas`, `ddobjects.pas` — the two largest units, about 9,600 and 20,000 lines) |
| Other | Splash screen, auto-update check (`autoupdatesthread.pas`), registry-backed settings and MRU lists, help integration, Developer Zone and feedback links |

Main menu: **File** (open, new, reopen, close table), **Connection** (wizard, new connection, repository toggle), **Database** (new dictionary, compare dictionaries), **Tools**, **Window** (tile, cascade), **Help** (contents, Developer Zone, about, feedback, check for updates).

## Connections and dictionaries

Connections live in a persistent Connection Repository tree; each node is either a data dictionary or a free-table directory, opened over Local, Remote or Internet (AIS) server types.

| Capability | Where | Notes |
| --- | --- | --- |
| New Connection Wizard | `newconnwizard.pas` | Four paths: existing dictionary, directory of existing tables, new dictionary, new database |
| Create / open connection | `ddopen.pas`, `ddframe.pas` | Path or alias, server type (Local, Remote, Internet), anonymous login, recent-dictionary dropdown |
| Aliases | `addalias.pas`, `adsini.pas` | Adds aliases to `ads.ini` |
| Login | `ddlogindialog.pas`, `passwordenter.pas` | User name and password per dictionary |
| Repository management | `ddmainscreen.pas` | Add to repository, rename, delete connection, connection properties, disconnect, reopen at startup, pick active connection |
| Create dictionary | `ddDictionaryDialog.pas` | Name, description, version (major/minor), AdsSys password |
| Dictionary properties: paths | same | Default and temp paths |
| Dictionary properties: security | same | Logins required, check user rights, enable internet access and security level, max login attempts (and enforcement on the remote server) |
| Dictionary properties: encryption | same | Encrypt new tables, table encryption password, encrypt dictionary files, encrypt indexes, encrypt communication |
| Dictionary properties: advanced | same | Disable triggers, allow queries and procedure calls via root dictionary, FTS defaults (delimiters, drop and conditional-drop characters, noise words) |
| Disable / enable logins | `ddDisableLoginsDialog.pas` | Locks out new database logins |
| Delete dictionary | `ddobjects.pas` | Removes the dictionary |
| Free a dictionary-bound table | `fixadttable.pas` | Strips the dictionary binding from an ADT |
| Environment Check Utility | `envcheck.pas` | Tests connectivity to a server path and reports settings and a call trace |

## Tables and indexes

ARC opens, browses, edits, designs and restructures tables in all five ADS formats: Advantage (ADT/ADI), FoxPro (DBF/CDX), Visual FoxPro (DBF/CDX), Clipper (DBF/NTX), plus auto-sense.

**Opening**

- Open Table dialog (`openwin.pas`): from a connection, dictionary or path; table type with auto-sense fallback (`autosensewin.pas`); ANSI or OEM character type; locking mode (proprietary or compatible); exclusive, read-only, check rights.
- Additional non-auto-open index files: CDX, NTX, ADI (`addindexes.pas`, `ntxdlg.pas`, `ntx_idx_dlg.pas`).
- Reopen recent tables (MRU), multi-table open, Browse Database tree window (`browsetree.pas`).

**Browse window** (`browsefrm.pas`)

| Group | Capabilities |
| --- | --- |
| Navigation | Grid with navigator, goto record number, find column, order by any index, incremental search (exact or partial) |
| Filtering | Set filter (expression), scope on the active order |
| Editing | Insert, edit, post, cancel, delete records, copy and paste records between tables, memo and image viewer (`image.pas`) |
| Deleted records | Show deleted DBF records, recall current or all records |
| Maintenance | Pack, reindex (with page-size choice, `pagesize.pas`), online pack and online reindex, empty (zap) |
| Encryption | Encrypt and decrypt table, table encryption passwords |
| Export | To new table, to existing table, and to HTML, Word, Excel, text, RTF, CSV, tab-delimited, DIF, SYLK or clipboard |
| Properties | Table properties popup, shrink columns, record count |

**Table Designer** (`tabledesignerf.pas`)

| Area | Capabilities |
| --- | --- |
| Fields | Add and delete fields; name, type, length, decimals; all ADT types (Char, CIChar, NChar, VarChar, NVarChar, Memo, NMemo, Blob, Image, Binary, Logical, Date, Time, Timestamp, Compact Date, ModTime, RowVersion, AutoInc, Short, Integer, Long, Double, CurDouble, Numeric, Money, GUID and others) |
| Field properties (dictionary tables) | Description, default value, minimum and maximum value, nullable, validation message |
| Indexes | Add and delete tags; expression, condition, unique, descending, candidate, custom, key type and length, page size; collation; "Add DELETED() index" helper |
| Full-text indexes | FTS index definitions with min and max word length, delimiters, noise words, drop and conditional-drop characters, case sensitive, keep score, protect numbers, compound, fixed |
| Table properties | Description, auto-create, primary index, default index, validation expression and message, encryption, memo block size, table caching, transaction-free, permission level |
| Restructure | Alter an existing table on a background thread with progress (`restructurethread.pas`, `Progressf.pas`), online operations, index-error recovery (`indexerrordlg.pas`), constraint-failure review with save, export or discard (`showrecordsF.pas`) |
| Schemas | Save and load designs, print the design or print from a template (ReportBuilder) |

**Other table operations** (repository tree): rename table (optionally the physical file, `renamefrm.pas`), move table file (`movetablefile.pas`), copy, delete with file removal (`FileDel.pas`), add existing tables to a dictionary, add existing index file, make primary or default index.

## SQL

The SQL Utility (`native_sql.pas`, opened as "Test Queries" or Tools → SQL Utility) is a script editor with execution, plans, transactions and a full SQL script debugger; the Query Builder adds a visual designer.

| Group | Capabilities |
| --- | --- |
| Editor | New, open, save, save-as SQL scripts; multi-file save prompt (`SaveMultiDialog.pas`); cut, copy, paste, select all; find, find next and previous, replace; syntax-coloured rich edit (`drmrichedit.pas`, `SafeRichEdit.pas`) |
| Execution | Execute query or script on a background thread (`querythread.pas`), cancel query, verify SQL without running it, query parameters (`queryparams.pas`) |
| Results | Grid with copy and paste records, find column, shrink column, default order; messages tab; export to new table, existing table, or HTML, Word, Excel, text, RTF, CSV, DIF, SYLK, clipboard |
| Transactions | Begin, commit, rollback from the toolbar |
| Execution plan | Show plan, render it graphically (`adssqlplan.pas`, about 2,150 lines), parse a plan from a result set |
| Options | Lock type (proprietary or compatible), source table type (ADT or DBF/CDX), rights checking, ANSI or OEM, request live, sequenced, max column display size, blank or last script on startup (`sqloptionsdlg.pas`) |
| Links | Link Manager: create and drop dictionary links by alias or path, authenticate active user (`sqllink.pas`) |
| Encryption | Table encryption passwords for the session (`encrpasswords.pas`) |
| Connections | Choose or switch the active connection, disconnect |

**Debugger** (`debugthread.pas`, `debugdatamodule.pas`, `debugbreakpointsf.pas`, `debugbreakobjf.pas`)

- Debug a query, or debug all new connections (catches calls from other applications).
- Step into, step over, step out, run to cursor, debug break.
- Set or clear a breakpoint at a line, or at a database object (procedure, function, trigger).
- Breakpoints, call stack, stack frame and variables panes.
- Entry points from the tree: Debug/Test on stored procedures, functions and triggers.

**Query Builder** (OpenQBuilder with an ADS engine, `qbuilder.pas`, `oqbeads.pas`): drag tables onto a canvas, draw joins with join type and operator (`qblnkfrm.pas`), pick columns, and generate SQL. BDE, IBO and FIB engines also ship in the source.

## Database objects

Every dictionary object type has a tree node with create, properties, rename, delete and Generate SQL Script actions, plus the type-specific operations below.

| Object | Dialog | Capabilities |
| --- | --- | --- |
| Tables | Table Designer, `tableproperpop.pas` | Create, add existing, open, properties, encrypt and decrypt, rename, move, triggers per table, export |
| Index files | tree | Add existing index file, make primary or default index |
| Views | `ddViewDialog.pas` | Name, description, view SQL, verify |
| Stored procedures | `ddStoredProcDialog.pas`, `spexecute.pas` | SQL script body with verify syntax; or external containers: Windows DLL or Linux shared object, COM or .NET assembly, AEP ProgID (`ProgidU.pas`); input and output parameters; varying output cursor; execute with parameters; debug and test |
| Functions | `ddFunctionDialog.pas` | User-defined SQL functions with parameters and return type, optional package, debug and test |
| Packages | `ddPackageDialog.pas` | Name and description; groups functions |
| Triggers | `ddtriggerdialog.pas`, `gettrigname.pas` | Table and database triggers; event (insert, update, delete) and type (before, after, instead of, on conflict); priority; SQL script, DLL or shared-object function, or COM/.NET method; include `__old`/`__new` VALUES tables and their memo/blob data; implicit transactions; disabled flag; verify syntax |
| Referential integrity | `ddridialog.pas`, `dderdialog.pas`, `ripropertiespop.pas` | Parent primary key and child foreign key, update and delete rules, fail table, custom error messages; RI Visual Designer (ER diagram) with drag-and-drop tables and printing across N×M pages (`printform.pas`) |
| Users | `ddUserAndGroupDialog.pas`, `userselect.pas` | Create, delete, password and change password, description, disable user, enable internet access, group membership, copy settings from an existing user |
| Groups | `ddUserGroupDialog.pas`, `ddGroupMembershipDialog.pas` | Create, delete, manage members |
| Permissions | `ddPermissionsDialog.pas`, `dbpermissiongrid.pas` | Grid per object; grant, grant with grant option, revoke; field-level permissions; effective permissions view |
| Links | `ddLinkDialog.pas` | Linked dictionary by alias or static path, authenticate active user or explicit user and password |
| Publications | `ddPublicationDialog.pas`, `ddPublicationWizard.pas` | Articles per table with row identifier columns, filter, column selection, SQL MERGE use for INSERT and UPDATE; wizard creates publication and articles in one pass |
| Subscriptions | `ddsubscriptiondialog.pas` | Publication, target database and connection type (remote or internet), credentials, queue table, forward replicated records, ignore failures, log failed updates; pause, resume, disable, view queue, process replication queues |

**Dictionary comparison** (Database → Compare Data Dictionaries, `dfSelect.pas`, `dfdiff.pas`, `dftreeview.pas`): pick two dictionaries, exclude objects, see a difference tree and side-by-side object views, then write a SQL script that turns one into the other, optionally using online operations.

## Utilities and tools

Beyond editing, ARC wraps import, code generation, backup and restore, server monitoring and maintenance; most server-side actions go through system procedures (`sp_BackupDatabase`, `sp_mgKillUser`, `sp_EncryptTable` and others).

| Tool | Where | Capabilities |
| --- | --- | --- |
| Import Data wizard | `import.pas`, `bdeimportoptions.pas` | Sources: OLE DB/ADO connection string, Paradox/dBASE, Advantage FoxPro, Advantage CA-Clipper, BDE alias, fixed-width text, delimited text (comma, tab, semicolon, other; quoted strings; header row); wildcards for many files; ANSI or OEM; import char fields as CIChar; minimum char size; no NULLs in new tables; create or append to destination |
| Text Import Assistant | `textimport.pas`, `textimport2.pas`, `texttable.pas` | Step-by-step field definition (name, type, width, decimals) for delimited or fixed-width files |
| Export Table Structures as Code | `tabletocode.pas`, `outputcode.pas`, `sqlwriter.pas` | Generates Delphi, C++ Builder or SQL scripts that recreate tables, indexes and FTS indexes for ADT, CDX, NTX or VFP and a chosen server type; optionally includes existing data; view, save or open in the SQL window |
| Generate SQL Script | tree context menu | DDL for any dictionary object or the whole dictionary |
| Backup | `backupdialog.pas`, `backupthread.pas` | Dictionary or free-table backup; destination, archive file, compressed; differential backup and "prepare for differential"; table mask or specific tables; free-table passwords; preview SQL and command line |
| Restore | `restoredialog.pas` | Source, destination, archive, password, specific tables, free-table passwords, preview SQL and command line |
| Remote Server Info | `mgtscrn.pas` | Installation info (revision, serial, registered owner, user limits, eval expiry); live statistics with auto-refresh (connections, work areas, tables, indexes, locks, worker threads, memory); connected users with disconnect; open files and lock owners; active queries with cancel; communication statistics with reset; configuration parameters (ports, log paths, sizes) |
| Table encryption | browse window, tree | Encrypt or decrypt tables, set dictionary encryption type, manage table passwords |
| Disable logins | `ddDisableLoginsDialog.pas` | Block new connections for maintenance |
| Free bound table | `fixadttable.pas` | Detach an ADT from its dictionary |
| Environment Check | `envcheck.pas` | Connectivity and client environment diagnostics |
| Replication queues | tree | Process replication queues, view queue, pause or resume subscriptions |

## Settings and UI

ARC Settings (`settings.pas`) and the SQL options are stored in the registry and cover display, connection defaults and startup behaviour.

| Tab | Settings |
| --- | --- |
| General | Date format (eight patterns, e.g. MM/dd/yy, dd.MM.yy, MM/dd/ccyy), number of decimals, always auto-sense table type, show deleted DBF records, preserve spaces in empty ADT char fields, open shell files in a new instance, automatic update check |
| User interface | Menu bar style (Flat, Enhanced, Standard, Office 11, XP), grid font size, alternate row colour, accurate scrollbar, autosize browse window, minimum column width, show memo data in grids, show record count, display-width limit |
| Connections | Reopen connections at startup, default connection properties, use defaults without prompting |
| Query | Blank or last-saved script at startup |

**Shell features**: MDI with tile and cascade, dockable Connection Repository, MRU lists, initial working directory for file dialogs (`workingdir.pas`), sort tree nodes by name or order, progress bar in the toolbar, shell-file association, embedded help browser, splash screen, update checker with skip and disable options.
