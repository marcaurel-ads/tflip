# TODO

What is left to do, with an effort estimate on each item. What tflip does
is summed up in the README's [status](../README.md#status), and what each
release added in [CHANGELOG.md](../CHANGELOG.md); completed work lives
there, not here — when an item lands, it is deleted from this file.
Where tflip differs from the original engine is listed in
[KNOWN_ISSUES.md](KNOWN_ISSUES.md); many items here link to their entry
there, which describes what the probes showed.

Every item follows the same rules: on-disk formats match the original
engine byte for byte, checked against goldens in `fixtures/`; behaviour is
found by probing the real engine (under Wine with `tools/ace/`, or on the
native Windows VM — see `LOCAL.md`), never by reverse engineering; and an
item is done only when the whole workspace's tests pass.

Effort: **S** ≈ hours, **M** ≈ a day or two, **L** ≈ several days, **XL** ≈
a sub-project. "Probe" means recording the engine's behaviour into a fixture
with a new `tools/ace/**` before writing any code. The estimates leave out
the plumbing each new API function needs (the ace wrapper, a protocol `api!`
entry with a `PROTOCOL_VERSION` bump and re-blessed wire goldens, an engine
`*_api`, the docs and changelog).

## Order of work

1. Read-ahead — measure first, and drop it if it does not pay.
2. The engine's tail.
3. Dictionaries; SQL in the order of its section.
4. What is left of encrypted dictionaries.
5. Server features: events, then replication last (the highest risk, and
   its engine is still to build).

The original's locks come when a client
needs them.

## Tests

- **Time-dependent output under a test clock** (**S**). The DBF header
  date (bytes 1–3) is handled: `clock::adopt_todays_dbf_date` lets a test
  accept today's or yesterday's date where the recorded file has its own,
  in `backup::*` (files and archive members) and
  `dictionary_write::rc4_replay`. Still direct `SystemTime::now()` calls:
  backup's archive mtime and differential stamp, the deploy log and the
  random seeds. Route them through `clock`, so a test clock can follow if
  exact time-dependent goldens are needed.
- `setters::tests::setters_on_cursors_match_the_original_engine` (and,
  once, a few `remote::` write tests) failed in a full `tflip-ace` run but
  pass alone and in repeated partial runs: find what they share under load.
  The setter test's local twin then reads its values back but finds
  `all_types.adt` unchanged after the disconnect; the remote twin gets
  5018 from `AdsGotoTop`.

## Functions that still fail in part

Functions that fail with `AE_NOT_IMPLEMENTED` (listed by group in
[KNOWN_ISSUES.md](KNOWN_ISSUES.md#functions-not-implemented)):

- **What is left of `AdsDDDeployDatabase`** (**S**): work out the rule by
  which the original drops a field's properties when the field has a
  validation message (it never keeps the message; with a default it also
  drops the minimum and maximum, and with two fields carrying all four it
  drops more; `fixtures/dictionary/deploy/w_*`), the limits of a DBF
  table's fields it drops (all of a Clipper or FoxPro table's, a Visual
  FoxPro character field's; `d_*`), and moving the records that break
  constraints of a table with a memo field (`d_memov`, `d_vfpmemov`: a
  fail table with its own memo file). All are probed into fixtures and
  refused with `AE_NOT_IMPLEMENTED`.

## Exports clients look for

Functions a shipped or published client declares that `tflip-ace` does not
export. A Delphi client links them `delayed`, so it starts without them and
fails only when the feature that calls one is used. Probe each against the
original before implementing it.

- **ARC's own declarations** (**M** in all): ARC's source declares four
  functions outside `ace.h` and `aceunpub.pas` —
  `AdsGetTableCreationString` (`arc_glbl.pas`, called from
  `TMainForm.OpenTable2` in `main.pas`), `AdsPrepareSQLNow` and
  `AdsGetPreparedFields` (`ddobjects.pas`, `TDDAView.GetFieldList`: a view's
  field list) — and the TDataSet units one more, `AdsDDSetTriggerProperty`
  (`ace.pas`, called by `TAdsDictionary.SetTriggerProperty` in
  `adsdictionary.pas`). Their signatures are in those declarations; what
  they return is for probes.
- **The DBCAPI** (**L**): the 35 `ads_*` functions of `dbcapi.h`
  (`ads_init`, `ads_connect`, `ads_prepare`, `ads_execute`,
  `ads_fetch_next`, `ads_get_column`, …), which ACE exports beside the
  `Ads*` API. The published Python driver `adsdb` (and `adsdb-django` on
  it) loads ACE with ctypes and calls nothing else but
  `AdsBeginTransaction`; the Ruby gem `advantage` (and the ActiveRecord
  adapter on it) is a C extension on `dbcapi.h`. Without them these drivers
  do not start. See [REFERENCES.md](REFERENCES.md#client-libraries).
- **The ADO.NET provider** (**S** to find out): run the common paths of
  `Advantage.Data.Provider` on tflip's DLL — `Open`, `ExecuteReader`,
  `DataAdapter.Fill`, `DataTable.Load`, `AdsCommandBuilder`, and
  `CommandType.TableDirect` with seeks — and implement each function the
  run reports missing (`EntryPointNotFoundException` names it).
- **The other closed-source bindings** (**S** each to find out): the ODBC
  driver, the OLE DB provider and the Crystal Reports driver are built on
  ACE too (the help's distribution pages for each list `ace32.dll` or
  `ace64.dll` among the files to install). Like the ADO.NET provider, they
  are checked by running them on tflip's DLL — an ODBC and an OLE DB
  client's connect, query, fetch and update, a Crystal report — and
  implementing what fails.

## Dictionaries

The writes build on the verified dictionary-write stack
(`set_object_slots`, `delete_records`, `rebuild_dictionary`,
`append_objects`, `crypto::wrap`, the slot maps of each kind).

- **The dictionary index when a leaf is nearly full** (**M**): the `.ai`
  is kept up to date key by key (an AES dictionary's too, in place) and
  matches in every recorded step but a few single-statement inserts
  (`CREATE TABLE` with a key, `CREATE USER`, `CREATE INDEX`, replication
  objects) where the original splits a nearly full leaf at another key
  than tflip, or tflip splits and the original does not
  (`fixtures/sql/ddapirights` line 62, `ddrepl`, `ddrenamerights`,
  `ddapirights2`, `spddalterfail` line 87, `spddalterfail3`; their `.ai`
  is not compared). It is not about deletes and inserts interleaving.
  **Probe**: keep the dictionary after each line of a script (prefixes of
  the script under `run.sh`) and replay each step on the previous `.ai`,
  to find the rule of a leaf's capacity and of the split position (exact
  fits are accepted in most places, but not at those two lines).
- **What is left of `DB:Public` slots that do not check** (**S**): the
  listings the original answers with endless or repeated rows (see
  KNOWN_ISSUES, Rights granted to `DB:Public`) are probed but not matched,
  among them an ordinary user's permissions and effective permissions on a
  dictionary with a bad table slot (`fixtures/sql/ddpublicbad10`, recorded,
  not replayed).
- **A one-character password's check** (**S**): **probe** whether the
  original accepts a wrong password that matches the one byte its stored
  check holds (tflip does, about once in 256 tries).
- **A failed drop of an order's field in a dictionary's table** (**S**):
  the original puts the objects in other slots and uses one id fewer than
  tflip (`fixtures/write/restructure_fts_dict_drop`, its test ignored);
  **probe** a failed drop on a table with several fields before and after
  the order's.
- **What is left of encrypted dictionaries** (**S**): the gaps of
  encrypted DBF tables and of VFP tables laid out for AES in KNOWN_ISSUES.

## The engine

- **NChar and NVarChar index orders** (**S**): left are the ~50 characters
  whose collation elements the probe could not tell (Arabic-Indic digits,
  some Tibetan letters, squared katakana words, ..., marked `?` in
  `icu_root.txt`); orders on an *expression* over such a field with
  `TRANSFORM` (which needs the text pictures of `TRANSFORM` first: `@!`,
  `@R`, `!`, `X`), `AT`, `REPEAT` with a count that is not a constant, or
  an `IIF` whose first branch is 8-bit (each still 5042);
  an NChar expression compared in a `FOR` condition (read as lossy 8-bit
  text); and the strings no probe covered:
  decomposed text whose marks need reordering, contractions with marks in
  between.
- **The original's locks** (**L**): sharing files with a running original
  engine (or Clipper and FoxPro programs) needs the byte ranges it locks,
  which another process can find by trying them. This also makes
  `AdsStmtSetTableLockType` real.

Kept as they are, since the original's local server does nothing either:
`AdsSetProperty`'s other ids, the callbacks (`AdsRegisterCallbackFunction`,
`AdsRegisterProgressCallback`, ...), `AdsMgKillUser`,
`AdsMgDumpInternalTables`, `AdsDisableLocalConnections`,
`AdsDisableRI`/`AdsEnableRI` (the rules always apply) and the client-side
`AdsSetRightsChecking`/`AdsStmtSetTableRights` (the server enforces rights
regardless).

## SQL

Everything the Advantage 12 help lists as SQL (its "Supported SQL
Statements", the script language, the scalar functions and the system
procedures), in the order to build it. What fails today, and how, is in
[KNOWN_ISSUES.md](KNOWN_ISSUES.md#sql). Each step first probes the original
with a `tools/ace/sql/*.txt` script (`run.sh`) into `fixtures/sql/`, and for
what writes files, records them under `fixtures/write/` to compare byte for
byte.

### 1. The small gaps (**M** in all)

- **What is left of full-text orders** (**M**): orders on Unicode fields
  (ICU sort keys); and `CONTAINS` on FoxPro and VFP tables, which tflip's SQL
  answers with nothing (see KNOWN_ISSUES), then its queries added to
  `fts_tx_cdx_*` and `fts_tx_vfp_*` (`tools/ace/write/fts_tx.py` leaves
  them out for DBF).
- **The SQL edge cases still known to differ** (**S** each, **L** in
  all): the 20 of `fixtures/sql/claims_known.txt` and
  `claims_huge_known.txt`, grouped in KNOWN_ISSUES; deep nesting wants the
  parser and evaluator off the stack's recursion.

### 2. Scripts (**M** each)

The interpreter (`tflip_sql::script`, hosted by `psm.rs` and `trigger.rs`)
runs procedure, trigger and function bodies; what is missing builds on it.

### 3. The larger pieces (**L** each)

- **Plans by cost**: the order of three tables or more in a join (the
  smallest first, the others probed) and the unexplained 12-against-10
  record join of `fixtures/sql/joinorder.json`, `ORDER BY` through an
  index's key, orders on expressions (`AOF SCAN` of `UPPER(a) = …`), an
  equality against an expression in a join's lookup, and operators that
  pass rows on one at a time rather than all at once. `SHOW PLAN` follows.
  `sp_CreateIndex90` ignores its collation argument.
- **What is left of external procedures and triggers** (`.aep`, the
  help's "The structure of AEPs", "Trigger Containers" and "DLL Caching"):
  the local server and `tflipd` run a procedure's library as the original's
  server does (`fixtures/sql/aep.json`, `tools/ace/aep/`;
  `crates/tflip-engine/src/aep.rs`, `crates/tflip-aep`). What is left:
  - `tflipd` on Windows running one (built, not tried: the library links
    tflip's `ace64.dll`, found as the module of that name);
  - `NO VALUES` of a script trigger (accepted, not probed);
  - trigger libraries on the remote server: only procedures are probed
    for transaction calls (`fixtures/sql/aeprulesremote.json`);
  - a library calling its ACE from a thread of its own, or after it
    returned, in `tflipd` (its calls reach the session only on the thread
    that called it, while it runs);
  - DLL caching (a temporary copy, loaded again when the file changes,
    with `Shutdown` and `Startup` around it; not in the local server) and
    `ADS_DD_DISABLE_DLL_CACHING`;
  - `AdsRegisterUDF` calling back (it is accepted but never calls).

  COM and .NET libraries (procedures, and `ASSEMBLY` triggers, which fail
  as a container that cannot load) and version 1 ones are left out.
- **The SQL debugger**: the commands besides `DEBUG BEGIN` and `DEBUG END`
  (breakpoints, stepping, watching variables), which ARC's SQL utility
  uses. Needs scripts that can stop between statements, so it comes after
  step 2. Probe what ARC sends and reads.

### 4. System procedures (**S** each, as clients ask for them)

Most wrap calls tflip has; each needs its result cursor's columns and its
errors probed. In families:

- **What is left of the dictionary procedures** (**M** in all): the validation options that
  check rows (the fail table), `sp_ModifyDatabase`'s `DEFAULT_TABLE_PATH`
  and `TEMP_TABLE_PATH`, and the publication, article and subscription
  procedures. See KNOWN_ISSUES.
- **The rest of management** (**S**): `sp_AllowMultipleCollations` (it
  rewrites `adscollate.adt` and `.adm`, leaving `.BAK` files, even when
  nothing matches: probe it on a private copy of the SDK folder only).
- Events and replication come with their
  [server features](#server-features).

## Remote server

- **Other handles' stale images** (**M**): the original's handle keeps
  the record image and count it read until it locks, refreshes or writes;
  tflip's handles see other handles' and a transaction's changes at once
  (records another handle deleted, appended or freed by a rollback).
  **Probe** what each call reads again (`fixtures/write/tx_cursor_after`
  covers only the handle that did the writing), and the stale count of an
  ADT handle after a transaction that also deleted a record through it
  (the original refreshes it at the commit).
- **ADT memo values on a shared table through the remote server** (**S**):
  the original puts every changed value in a new block, outside a
  transaction and with one handle alone; tflip writes it in place.
  **Probe** the block it takes (the first draft of `memo_other_handle` had
  ADT cases `ta` to `tl`).
- **Read-ahead across records** (**M**): `AdsCacheRecords` (a block per
  table, default 10, 0 or 1 off; edits and `AdsRefreshRecord` drop it;
  cached records do not show other users' changes), and caching memo values.
  A whole record is already read in one round trip, so measure ARC over a
  real network before building it.

## Server features

- **Replication that runs** (**XL**), tflipd to tflipd only (decided
  2026-10-19: replicating with an original server needs its wire protocol).
  The metadata is done; nothing runs yet. Three parts, **L** each:
  - change capture: a queue row for each committed change to a published
    table (the transaction log already holds the before and after images;
    the queue's format is tflip's own);
  - a worker that drains the queue and applies it;
  - a transport from one `tflipd` to another, replaying the changes over
    tflip's protocol and its existing write requests.

  `AdsMgKillUser` and the other management calls only mean something once
  `tflipd` serves many clients this way.

## What the documentation says

What the Advantage 12 help and the archived forums (see
[REFERENCES.md](REFERENCES.md)) describe that no probe has looked at yet,
read through on 2026-10-28. The documentation is a hint only: each item is
**probed** before anything is built on it, and what the probe shows goes to
KNOWN_ISSUES or ADS_BUGS. The gaps already certain (grammar the parser
lacks, ignored API options) are in [KNOWN_ISSUES.md](KNOWN_ISSUES.md); they
are built as part of the items below.

### Triggers, transactions and rights (**M** in all)

- **Database triggers that log** (v12): `OPEN_TABLE` and `CLOSE_TABLE`
  fire as the original's for statements, client handles, disconnects and
  the cases of `fixtures/sql/trigdblog*.json`. Left: the order a statement
  closes several tables in (the original's plan's; tflip's is the order
  opened, reversed for a `SELECT` with no condition), `OpCount` (the
  original's is the requests it served; exact only at `CONNECT`), the
  moments the tables a table trigger's statements open close, and a
  procedure's cursor holding its table to the procedure's end.
- **Triggers**: one created on another connection while a statement holds
  the table open, which the original's one open instance of the table does
  not see, for any connection, until no connection has the table open (a
  connection keeps a table its statements closed until it disconnects;
  `tools/ace/sql/txtrig3.txt` locally, `fixtures/events/events_trigger_open.json`
  on the remote server, replay ignored), where tflip sees it once the
  statement closes; a drop meanwhile stops the trigger at once on the remote
  server but is never seen on the local one. The firing order of database
  triggers, probed no further than newest first.
- **Transactions**: 7130 and 7187 were tried again with nested triggers,
  procedures, scripts and unended transactions
  (`fixtures/events/events_tx_sub.json`) and not reproduced; see
  TRANSACTIONS.md.
- **Rights**: the gaps of column rights in KNOWN_ISSUES.

### For the server features above

- **Events**: events across the processes of a local server (the original polls a shared
  events table).
- **Replication**: no triggers fire on the target but `ON CONFLICT`, for
  updates and deletes, when the target record's CRC (without `ROWVERSION`
  and `MODTIME`) differs; `PERMITTED_REP_ERRORS` (7137 by default);
  `sp_ProcessReplicationQueues(1 | 2)`.
- **`tflipd`**: `adsserver.ini`'s `[ServerAliases]` and the `__rootdd`
  alias; the error log `ads_err.adt` (the last 1000 errors).

### Bugs the forums report (**S**)

Probed in version 12, and recorded in ADS_BUGS. Left: the partial key seek
of `Céli` missing `Céline` under a VFP collation, compared with an order
tflip makes, once `AdsCreateIndex61` after `AdsSetCollation` is known to
make a collated order in tflip (see KNOWN_ISSUES).

## Unclear whether doable

Without reverse engineering, only documentation and black-box behaviour
can be used (see [REFERENCES.md](REFERENCES.md)). It is open whether that
is enough for:

- **The original wire protocol** (**XL**), which would let original
  Advantage clients use `tflipd`, and replicate with or find original
  servers.
