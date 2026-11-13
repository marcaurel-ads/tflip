<div align="center">

```
(╯°□°)╯︵ ┻━┻
```

# TableflipDB

</div>

TableflipDB (`tflip`) is an open-source database server that aims to be
compatible with applications written for Advantage Database Server. A
hobby project, written in Rust, with no schedule.

It is not a re-implementation of the Advantage wire protocol. Instead, it
replaces the client library: applications keep calling the Advantage Client
Engine API, but the `ace32.dll` / `ace64.dll` they load is tflip's, which reads
the tables itself or talks to the tflip server (`tflipd`) over tflip's own
protocol. The first goal is to run Advantage Data Architect (`arc32.exe`) on
tflip's `ace32.dll`.

## Status

Advantage Data Architect runs on tflip's DLL, locally and against
`tflipd`. tflip:

- **reads** ADT tables with every field type, and Clipper, FoxPro and
  Visual FoxPro tables, with their memo files and their ADI, NTX, CDX and
  IDX indexes: records in record or key order, seeks, scopes, filters and
  AOFs optimized as the original optimizes them;
- **writes** them as the original engine does, byte for byte: appends,
  changes, deletes, locks, memo values and index maintenance; it creates,
  restructures, packs, zaps, reindexes, copies and converts tables and
  indexes;
- **reads encrypted tables** with the password, in both schemes: the default
  RC4 and AES; records, memo values and index orders alike, a free table or
  one bound to a data dictionary;
- **encrypts and decrypts tables** (ADT and DBF) in the RC4 (Advantage
  proprietary) scheme as the original engine does, byte for byte: `AdsEnableEncryption`,
  `AdsEncryptTable`/`AdsDecryptTable`, the record-level
  `AdsEncryptRecord`/`AdsDecryptRecord`, and `AdsDisableEncryption`, with writes
  to an encrypted table enciphered (records and memo values);
- **reads data dictionaries**: users, tables and views by name, every
  object's properties, and — given the administrator's or a user's password —
  the slots the dictionary keeps encrypted (passwords, group memberships and
  permission grants), and an encrypted dictionary itself, RC4 or AES (with
  its dictionary password);
- **writes data dictionaries** as the original engine does, byte for byte:
  it creates one; adds tables, views, index files and stored procedures;
  creates triggers and referential-integrity rules; creates and deletes users
  and groups with their membership; grants and revokes permissions on every
  object type; removes objects and renames views; frees a table; and sets
  objects' comments and a few properties; AES dictionaries too, created with
  `CREATE DATABASE … ENCRYPTIONTYPE AES128 | AES256` and written as the
  original writes them, compared deciphered;
- **runs SQL** on ADT and DBF tables: `SELECT` with joins, grouping,
  `UNION` and subqueries, live and static cursors, `SHOW PLAN`, `INSERT`,
  `UPDATE`, `DELETE`, `CREATE TABLE`, `CREATE INDEX`, `ALTER TABLE`, and
  the system procedures ARC calls;
- **runs external procedures and triggers** in a library (`.aep`), on the
  local server and in `tflipd`: a version 2 library's procedures and
  trigger functions, with `__input`, `__output`, `__error`, `__new` and
  `__old` through SQL and the table API, as the original's server runs them;
- **backs up and restores** dictionaries and free tables online in
  `tflipd`, full and differential, into folders or tar archives, as the
  original's server does (`sp_BackupDatabase`, `sp_RestoreDatabase`,
  `sp_BackupFreeTables`, `sp_RestoreFreeTables`);
- **serves** folders to clients over TLS with `tflipd`.

The DLL exports all 465 functions of `ace.h` and the unpublished
interface, for 32-bit and 64-bit clients; none is a stub, though a few
settings, callbacks and statement passwords are accepted and ignored (see
[KNOWN_ISSUES.md](docs/KNOWN_ISSUES.md#functions-not-implemented)).
Dictionary writing is otherwise in place: `AdsDDCreate` makes one; `AdsDDAddTable`, `AdsDDAddView`,
`AdsDDAddIndexFile` and `AdsDDAddProcedure` fill it; `AdsDDCreateTrigger`
and `AdsDDCreateRefIntegrity` add triggers and referential integrity; the
replication functions add publications, articles and subscriptions (with
their replication queue table); the user, group and permission functions
manage access (including `AdsDDSetObjectAccessRights`); and the remove,
view-rename, modify-link and `Set*Property` functions prune it and set
properties. A client can create a **free** (non-dictionary) AES table through the API: a
connection whose `EncryptionType` is `AES128`/`AES256` lays a created table out
for AES, `AdsEnableEncryption` arms it (and reads one back), and its records,
memo values and index pages encipher as the original engine's, verified
byte-for-byte and end-to-end both ways against an OpenSSL-shipping engine build.
A **dictionary-bound** AES table is made as the original makes it in an AES
dictionary, armed under the dictionary's key once it encrypts new tables. The
full-text `CONTAINS` and `SCORE` run in SQL, over a field's full-text
index and with a non-indexed fallback; full-text indexes are created
(`CREATE INDEX … CONTENT`, `AdsCreateFTSIndex`), kept current and rebuilt
as the original engine writes them, byte for byte.
Transactions hold
their writes back as the original's server does, also across processes
and in SQL's autocommit modes, with a log on disk from which a crash is
recovered. What differs from the original is in
[`docs/KNOWN_ISSUES.md`](docs/KNOWN_ISSUES.md), what is next in
[`docs/TODO.md`](docs/TODO.md), and what each release added in
[`CHANGELOG.md`](CHANGELOG.md).

## Layout

| Path | Contents |
| --- | --- |
| `crates/tflip-storage/` | Reader and writer for tables, memo files, indexes and data dictionaries (`.adt`, `.adm`, `.adi`, `.dbf`, `.ntx`, `.cdx`, `.add`, ...) |
| `crates/tflip-expr/` | The expression engine: filter and index expressions, compiled and evaluated as the original engine does |
| `crates/tflip-sql/` | Advantage SQL: statements parsed, typed and evaluated as the original engine does, and planned as live or static cursors |
| `crates/tflip-engine/` | The engine behind the client library and the server: sessions, connections, handles, tables, index orders |
| `crates/tflip-protocol/` | The engine's API as the client library uses it, and its wire format and TLS between client and server |
| `crates/tflip-ace/` | The client library, exporting the Advantage Client Engine API as a thin layer over the API, for local and remote connections |
| `crates/tflip-aep/` | Loading external procedures' libraries and calling them, for the local server and `tflipd` |
| `crates/tflipd/` | The server |
| `crates/tflip-studio/` | A web-based admin tool for tflip, to replace Advantage Data Architect (see `STUDIO_PLAN.md`) |
| `fixtures/` | Tables and indexes written by the original engine, with JSON dumps of their contents; see [`fixtures/README.md`](fixtures/README.md) |
| `tools/ace/fixtures/` | The SQL scripts the fixtures were made from |
| `tools/ace/` | Probes that record what the original engine does (expressions, filters, SQL, DBF tables, data dictionaries, writes, ...), run under Wine |
| `xtask/` | `cargo xtask dist`: builds `ace32.dll` and `ace64.dll` |
| `docs/` | The documentation below |

## Architecture

Each layer only uses the ones below it:

1. **`tflip-storage`** knows the files: their formats, as the fixtures show
   them.
2. **`tflip-expr`** and **`tflip-sql`** compile and evaluate expressions
   and SQL.
3. **`tflip-engine`** is the engine: sessions and the objects behind
   handles (connections, tables, index orders, statements, listings), and
   every rule of how the original engine behaves that is not about ACE's
   calling conventions. It is plain, safe Rust, with no pointers, code
   pages or ACE error codes.
4. **`tflip-protocol`** is the engine's API (`Api`), implemented by an
   engine session in the process and by the client of a `tflipd` server,
   with the wire format between them. The client keeps the answers that
   cannot change while their handles are open. A test guards the wire
   format against changes without a new protocol version.
5. **`tflip-ace`** is the DLL: it turns each export into calls of the API,
   locally or on a server by the handle's value, and does only what is
   ACE's own. That means pointers and buffers, the ANSI, OEM and UTF-16
   text of the client, ACE's constants and error codes, the last error, and
   the exports' quirks of lengths and output. It keeps no state of the
   engine's objects; what a client keeps on its own (passwords, which
   server a handle is on) stays with it. Common exports make one call, so
   one request to a server; `round_trips.rs` checks it.
6. **`tflipd`** serves engine sessions to clients over TLS.

**`tflip-aep`** stands beside the DLL and the server: it loads an external
procedure's library and calls it, for the engine, which only knows a
trait. In `tflipd` the library's copy of tflip's client library sends its
calls to the client's session in the process, on the thread that called
it.

## Building

```sh
cargo test
# The client DLLs, ace32.dll and ace64.dll, into target/dist (needs the
# i686-pc-windows-gnu and x86_64-pc-windows-gnu targets and MinGW):
cargo xtask dist
```

See [docs/PLATFORMS.md](docs/PLATFORMS.md) for what runs where, and what is
specific to Windows.

## Running the server

```sh
cargo run --release -p tflipd -- --share data=/srv/tables
```

`tflipd` listens on port 6262 and prints the SHA-256 fingerprint of its
certificate, which it makes on its first start. Every connection uses TLS
1.2 or 1.3; clients trust a server by that fingerprint, given in
`TFLIP_SERVER_FINGERPRINT` or in `tflip.ini` next to the DLL:

```ini
[servers]
db1 = sha256:3f1c...
```

Given a certificate from a CA instead (`--cert` and `--key`), `tflipd` needs
no fingerprint on clients whose system trusts the CA (on Windows, its
certificate store); a private CA's roots can be named in `tflip.ini`'s
`[trust] roots`.

A client then connects to `\\db1\data\folder` (or `\\db1:6263\data\folder`)
with the remote server type. A `[connection]` section bounds talking to a
server — `connect_timeout` (default 5 s) and `request_timeout` (off by
default, so a stalled server does not block a client), both in seconds, and
`compression` (`INTERNET`, `ALWAYS` or `NEVER`, for connect calls that do not
say; `tflipd --compression` is the server's side). `[cache] local = yes` makes
the local server keep closed tables open for reuse as the original's local
server does (`AdsCacheOpenTables`, `AdsCacheOpenCursors`); it is off by
default, so that a table a client closed can be opened exclusively by another
program at once. A
dictionary connection's login password is
checked (the client sends it over TLS and the server verifies it), a
dictionary that verifies access rights enforces them for the connected user, and
its referential integrity rules are enforced on every write. A
free-table share, though, takes any user and enforces nothing — so anyone who
can reach the port and open such a share can read it.

An external procedure's or trigger's library that `tflipd` runs must link
tflip's client library (`libtflip_ace.so` on Linux, `ace64.dll` on
Windows), not the original's; on Linux it is a shared object under the
name the dictionary gives. See
[`docs/DIFFERENCES.md`](docs/DIFFERENCES.md).

## Documentation

| File | Contents |
| --- | --- |
| [`docs/DESIGN.md`](docs/DESIGN.md) | Why the engine sits behind a thin client library, what that costs remotely, and what tflip does about it |
| [`docs/C_API.md`](docs/C_API.md) | Proposal: `libtflip`, a lean, batched C API beside the ACE drop-in, with Arrow results |
| [`docs/DIFFERENCES.md`](docs/DIFFERENCES.md) | How tflip differs from Advantage Database Server, on purpose and not |
| [`docs/KNOWN_ISSUES.md`](docs/KNOWN_ISSUES.md) | The differences not on purpose: bugs and what is missing |
| [`docs/ADS_BUGS.md`](docs/ADS_BUGS.md) | Bugs, quirks and environment artifacts of the original engine, found by probing it |
| [`docs/TODO.md`](docs/TODO.md) | What is left to do, the next steps first |
| [`CHANGELOG.md`](CHANGELOG.md) | What each release added |
| [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) | How to work on tflip: probing the original engine, recording fixtures, tests and checks |
| [`docs/PLATFORMS.md`](docs/PLATFORMS.md) | What runs where, and what is specific to Windows |
| [`docs/TRANSACTIONS.md`](docs/TRANSACTIONS.md) | Transactions: what the original does, probed and documented, and how tflip does it |
| [`docs/REFERENCES.md`](docs/REFERENCES.md) | The external sources the reimplementation draws on, and what is excluded |
| [`fixtures/README.md`](fixtures/README.md) | The fixtures: what each covers, and how they were made |

## What is not included

tflip contains no Advantage software or documentation. Runtime files that
belong to an Advantage installation, such as the collation files, are read from
the user's own installation and are not distributed here.

## Trademarks

Advantage Database Server is a product of SAP SE. tflip is not affiliated with,
endorsed by or supported by SAP. Names of products are used only to describe
compatibility.

## License

MIT, see [LICENSE](LICENSE).
