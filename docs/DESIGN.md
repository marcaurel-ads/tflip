# Design notes

Why tflip is built the way it is, and what that costs. The layers are in the
README's "Architecture"; this file is about the reasons behind the line
between them, and about remote performance.

## One engine, a thin client library

Everything that decides what a table, an index or a record holds lives once,
in `tflip-engine` and the crates below it: the file formats, index keys,
NULL and empty values, expressions, record visibility, locking, SQL. The
client library (`tflip-ace`) only translates the ACE API: pointers and
buffers, the client's ANSI, OEM and UTF-16 text, ACE's constants and error
codes, the last error, and the exports' quirks of lengths and output. It
keeps no state of the engine's objects.

The local mode and the server run the same engine, in the process or in
`tflipd`. So there is one implementation of each rule, and the local and
remote modes cannot disagree about a key or a record: a client never builds
on its own something the server must later accept byte for byte. The
protocol between them carries the engine's API (`tflip-protocol`'s `Api`),
not file pages or record images. That keeps it small and lets it be
versioned: a test guards the wire format against changes without a new
protocol version.

## The cost: the API moves one record at a time

The ACE API is navigational. An application moves (`AdsSkip`, `AdsSeek`,
`AdsGotoTop`) and then reads or writes the current record's fields one call
at a time. With the engine behind a network, every call that needs the
engine is a request, and a scan of N records costs at least N round trips.
On a LAN that is cheap; over a WAN the latency dominates.

What tflip does about it today:

- **At most one request per common export.** `tflip-ace`'s `round_trips.rs`
  counts the requests of the common exports (field reads, moves, seeks,
  record counts, key positions, writes, locks) against a budget of one, after
  warming the caches, and fails when an export goes over it.
- **The current row is read whole.** The first field read after a move
  fetches every field of the row in one request, and the other fields of that
  row are answered from the client (`Client::field_as_cached`). Memo values
  and unknown fields are read on their own.
- **Answers that cannot change are kept.** The replies to requests whose
  answers stay fixed while their handles are open (a table's structure, for
  example) are kept by the client. A request that closes objects or opens an
  index file makes the client forget all of them.

What it does not do yet:

- **Read-ahead across records.** The documented `AdsCacheRecords` (the number
  of records to read ahead) is accepted and ignored: tflip reads one record
  per move. Records read ahead go stale when other users change them, so a
  read-ahead block has to be dropped on every edit, `AdsRefreshRecord` and
  lock, and must not hide other users' changes where the application expects
  to see them. That is client-side state again, the kind the thin client
  avoids, so it waits for a measurement of a real client (ARC) over a real
  network ([TODO.md](TODO.md), "Read-ahead across records").

## The set-based path

SQL and server-side filters do not have this cost: a statement or a filter
runs entirely in the engine, and only its result crosses the network. For
remote use, an application that scans many records does better with a
`SELECT` or a filter than with a loop of `AdsSkip` and `AdsGet*`. tflip
cannot change how existing applications call the API, but it keeps the
set-based paths complete and cheap so that they remain the fast route.

## Compatibility fixes the behaviour, not the internals

tflip must run unmodified Advantage applications, so what they can observe
is fixed: return codes, the last error's text, lengths and buffer behaviour,
and the bytes of every file it writes. How tflip gets there is its own. The
differences it accepts on purpose are in [DIFFERENCES.md](DIFFERENCES.md);
the ones it does not are in [KNOWN_ISSUES.md](KNOWN_ISSUES.md).
