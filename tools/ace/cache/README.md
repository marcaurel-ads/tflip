# Open-table caches: black-box findings

What `AdsCacheOpenTables`, `AdsCacheOpenCursors` and `AdsCloseCachedTables`
do in the original engine (12.0 SDK `ace32.dll`), from driving them under Wine
against the evaluation server (under Wine, setup in `LOCAL.md`) and the
local server; **no disassembly**. The output is kept in `fixtures/cache/`.

    tools/ace/cache/run.sh <work folder> <acesdk folder> [remote]

## Findings (2026-10-23)

- **Answers.** Both setters take any `UNSIGNED16` (0, 1, 5, 250, 65535) and
  answer 0, with or without a connection open. They take no handle: the
  setting is the process's, and it applies to connections already open.
  `AdsCloseCachedTables(0)` answers 5024 (`AE_INVALID_CONNECTION_HANDLE`, last
  error 5018); a made-up handle crashes the client.
- **Defaults.** No table is kept by default; the tables of closed `SELECT`
  statements are (25 by the help, not counted here).
- **Per connection, oldest first.** With N = 2, a connection that closes `t`,
  `u`, `v` keeps `u` and `v` open; another connection keeps its own N.
- **Kept tables show as open** (`AdsMgGetOpenTables`) but hold no lock: a
  table closed while locked (`AdsLockTable`) lets another connection lock it.
- **They yield.** An exclusive open of a kept table, by another connection or
  the same one, succeeds, and the table is no longer kept. `DROP TABLE` of a
  kept table succeeds, and closes **every** kept table (not only the one
  dropped).
- **Closing them.** `AdsCloseCachedTables(c)` closes `c`'s kept tables (open
  tables stay open), not another connection's, and not its statements' tables;
  `AdsCloseAllTables` closes every connection's kept tables; lowering
  `AdsCacheOpenTables` to 0 closes them; disconnecting closes them.
- **Statements.** A statement holds the table its cursor read until the
  statement closes (even after the cursor closed); with a cursor cache it stays
  open after that, yields to an exclusive open, and a repeated `SELECT` sees
  other connections' inserts. `AdsCloseCachedTables` leaves it; lowering
  `AdsCacheOpenCursors` to 0 leaves it on the remote server but closes it on the
  local one; disconnecting closes it.
- **Statements hold tables** (2026-10-23, section "statements hold tables").
  After its cursor closed, an open statement's table is listed open, another
  connection's exclusive open of it fails (7008), `DROP TABLE` of it fails
  (7200, native 7008), and `AdsCloseCachedTables` leaves it. Running the
  statement again on another table lets go of the first (with no cursor
  cache, it closes); closing the statement with its cursor still open closes
  both. With a cursor cache of 5 the exclusive open fails the same way until
  the statement closes, then succeeds. Remote and local alike.
- **The local server caches too**, the same way (`fixtures/cache/local.txt`);
  so the caches are not a remote-only feature.

See `docs/KNOWN_ISSUES.md` for where tflip differs.
