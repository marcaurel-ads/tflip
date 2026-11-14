# Differences from Advantage Database Server

tflip aims to behave as SAP's Advantage Database Server and its client
engine do, as far as clients can see: the API in the SDK's `ace.h` (and the
unpublished declarations of the SDK's `aceunpub.pas`), with the results,
error codes and edge cases the original engine shows when probed. Where it
differs, it is either on purpose, listed here with the reason, or not, listed
in [KNOWN_ISSUES.md](KNOWN_ISSUES.md). Bugs, quirks and environment artifacts of
the original engine itself — where there is no correct behaviour to match — are
in [ADS_BUGS.md](ADS_BUGS.md).

The behaviour of the original engine is learnt from its documentation and
from calling it (black-box probes under Wine). Its binaries are not
inspected.

## On purpose

### Architecture

- **The client library is replaced, not the server.** Clients load tflip's
  `ace32.dll` or `ace64.dll` in place of the original. It reads tables
  itself (the local server) or talks to `tflipd`. The original's remote
  layer (`axcws32.dll`) is not replaced: what `ace32.dll` hands it is
  undocumented.
- **tflip's own protocol.** `tflipd` and tflip's DLL talk over a protocol of
  their own, not the original's wire protocol. Original Advantage clients
  cannot use `tflipd`, and tflip's DLL cannot use an Advantage Database
  Server; whether the original protocol can be learnt without reverse
  engineering is open.

- **External procedures in `tflipd` must link tflip's client library.** An
  `.aep` calls back through the ACE it links with the handle it is given;
  in `tflipd` that ACE must be tflip's (`libtflip_ace.so`, `ace64.dll`),
  which sends the calls to the client's session in the process. A library
  that links another fails with 5154. On Linux the library is a shared
  object under the name the dictionary gives (the help's advice: keep the
  `.aep` name); its failures to load or to find a function report the
  Windows error codes the original's do (126, 127), so that clients see the
  same text. COM and .NET libraries, version 1 ones, and DLL caching are
  not supported. The handle a library is given is a remote connection's to
  `AdsGetConnectionType` (2), where on the original's local server it is a
  local one's (1).

### Security

- **TLS is mandatory** between a client and `tflipd`: TLS 1.2 or 1.3, with
  no plain-text mode. The original made TLS optional and supported it only
  up to TLS 1.0. A connect string that asks for a non-TLS transport
  (`CommType=TCP_IP`, `UDP_IP` or `IPX`) is refused with `AE_INVALID_OPTION`
  rather than silently given TLS, unless `tflip.ini`'s `[connection]
  commtype = ignore` (or `TFLIP_COMMTYPE=ignore`) says to connect over TLS
  all the same; its other TLS settings (the cipher list,
  certificate, common name, FIPS mode) are accepted but not applied, since
  tflip picks its own TLS 1.2/1.3 ciphers and trusts servers by fingerprint
  or by the system's certificate store.
- **Servers are trusted by fingerprint or by the system's roots.** A client
  trusts `tflipd` by the SHA-256 fingerprint of its certificate, from
  `TFLIP_SERVER_FINGERPRINT` or `tflip.ini` next to the DLL (`tflipd` makes
  itself a self-signed certificate), or by a certificate from a CA that the
  operating system's store (or `tflip.ini`'s `[trust] roots`) trusts and that
  names the server. The original uses its own TLS settings in `ads.ini` and
  the connect string (`TLSCertificate`, `TLSCommonName`).
- **Compression is configured in `tflip.ini`, not `ads.ini`.** A client
  whose connect call gives no compression setting takes `[connection]
  compression` from `tflip.ini` (or `TFLIP_COMPRESSION`), where the original
  reads `ads.ini`'s `COMPRESSION`; the server's setting is `tflipd
  --compression`. The compression itself is tflip's own (deflate, one stream
  each way, under TLS), as is the protocol.
- **The local server keeps no closed tables open unless told to.** The
  original's local server keeps them as its remote one does
  (`AdsCacheOpenTables`, none by default; `AdsCacheOpenCursors`, the tables of
  25 statements by default), so a client's closed tables stay open in its
  process and another program cannot open them exclusively until the client
  closes its cached tables or its connection. tflip's local server keeps none
  by default, so a closed table is closed, and accepts and ignores both
  settings; `tflip.ini`'s `[cache] local = yes` (or `TFLIP_LOCAL_CACHE=yes`)
  makes it cache as the original's does (`fixtures/cache/local.txt` replays
  then). `tflipd` always caches, as the original server does.
- **A server reaches only its shares.** `tflipd` serves folders it was given
  (`--share NAME=DIR`); clients name their data by share
  (`\\server\NAME\folder`). Paths that leave the share (`..`, absolute
  paths) fail with `AE_FILE_NOT_ON_SERVER`, and so do free tables (opened
  on connection 0), which could name any file. `tflipd --map PATH=NAME`
  lets clients keep the original server's absolute paths: one under `PATH`
  (`C:\data`) is the same path in the share `NAME`, free tables' too.
- **Strong encryption is always available.** `AdsSetProperty`'s getter 87
  answers 1 on every handle, as the original does with its AES DLLs
  (OpenSSL) beside it; the stock original answers 0. tflip creates and
  reads AES tables without extra DLLs.
- **Unread bytes in encrypted files are not the original's leftovers.**
  The bytes the original leaves after an enciphered AES memo value, and
  those it enciphers with the header of a freed run in an AES memo file,
  are whatever its buffer last held; tflip writes zeros after a value and
  leaves the rest of a freed block as it was. No read sees these bytes, and
  copying another value's leftovers into a file would only leak data.
- **Random frames are drawn anew only where tflip writes them anew.** When
  a dictionary is converted (`sp_SetDDEncryptionType`), the original
  enciphers the rights granted to `DB:Public` and a user's membership of a
  built-in group again under a new random frame; tflip keeps the frame
  they had. Both decipher to the same rights; the frame is random in the
  original, so its bytes cannot be matched anyway.
- **Null pointers fail.** A null output pointer fails with
  `AE_INVALID_POINTER` where the original writes through it or crashes
  (`AdsGetColumnPermissions`, for one).

### Paths and names

- **Relative names are relative to the connection.** A relative table
  name or table listing mask (`*.adt`, `sub\*.adt`), or an index file name
  with a folder in it (`data\x.cdx`), is taken relative to the
  connection's folder. The original resolves it against the process's
  current directory, even on a connection (an index file named alone is in
  the table's folder in both, `fixtures/errors/index_paths.json`).
  `tflip.ini`'s `[paths] relative = cwd` makes the local server take them
  as the original does; `tflipd`'s clients have no current directory there.
- **`tflipd` ignores case** in share, folder and table names, as Windows
  file systems do, so that clients find their tables on Linux.
- **Backup and restore paths** are paths on `tflipd`'s host, as they
  are on the original's server; a drive letter on a host that is not
  Windows names no folder (7040, a code 3 row).

### Handles and values

- **Handles are not reused.** A closed handle stays unknown rather than
  reaching a newer object; the original reuses values. Handles of
  connections to `tflipd` come from ranges of their own (from
  `0x40000000`). Clients cannot rely on either.
- **The handle in an order's bookmark is tflip's own**: the original's
  bookmark through an order holds the order's handle, and tflip's holds its
  own handle number there. Bookmarks are opaque to clients.
- **`AdsFindServers` with no servers returns an empty table**, not an error.
  tflip returns `AE_SUCCESS` and a zero-row result cursor, which is the
  documented `phTable` contract ("one row for each address discovered"). The
  original returns the system error `50` — not an Advantage error code — and a
  NULL handle in that case (a defect; see `docs/ADS_BUGS.md`). When servers
  answer, both return the populated 13-column table.
- **A double's 17th digit on a tie is rounded to nearest.** The text of a
  Double or CurDouble is its value to 17 significant digits, rounded to the
  field's decimals, in both (1.5e30 is `1499999999999999900000000000000`).
  Only where the value's 18th digit is a 5 with nothing after it (or nearly)
  does the original round the 17th digit its own way: 209409263140909.375 is
  `...909.37` there, 16.7268524169921875 `16.726852416992187`. Its choice
  depends on the value and its decimal exponent, as an inexact scaling in its
  C runtime's extended-precision arithmetic would, and no plain model of that
  matched the ~1000 ties probed (`tools/ace/fields/doubles.c`). tflip does
  not emulate the old C runtime's arithmetic; it rounds such ties the
  ordinary way.
- **A dictionary default of a type its field does not take stays unset.**
  The original lets a number or a logical value be a Date field's default,
  and a timestamp a Money field's, and fills in a value no reading of the
  default explains (`1+1` is 1900-01-01, `1.6` 1900-01-06, `1=2`
  1900-01-02; `NOW()` a Money of some 24 trillion), where the other numeric
  and date fields refuse such a default (see `docs/ADS_BUGS.md`). tflip
  takes the same defaults but, finding no value in them, leaves those fields
  NULL.

### Text

- **Error descriptions** are the original engine's texts
  (`fixtures/errors/texts.json`), and SQL errors (7200) are worded as its;
  only codes it has no text for get a short description in tflip's words.
- **The original's wrong answers are not copied** where they are faults,
  not behaviour clients build on (see [ADS_BUGS.md](ADS_BUGS.md)): `WHERE
  YEAR(t) = …` over an order on a TimeStamp column finds its rows, where
  the original finds none; a syntax error's place is the true one after an
  apostrophe in a comment, where the original's is moved or missing; and
  `sp_CreateReferentialIntegrity` over tables with orders under a dynamic
  collation works on a connection without that collation, where the
  original fails with 7008.
- **Waits for events** last as long as their timeout says and end at once
  on a signal. The original waits in polls of 300 ms: ceil(timeout/300) of
  them on the local server, one more on the remote server (a timeout of 100
  ms takes 300 and 600 ms), and answers a signal at once only on the remote
  server. A negative timeout other than -1 waits forever here, as on the
  local server; the remote server waits 300 ms for -2 to -300 and,
  apparently, forever below. The poll interval argument is ignored, as the
  remote server ignores it.
- **Query logging's state** is kept per data folder (free tables) or
  dictionary, for the process, and `sp_ViewQueryLogging` lists the
  connection's own log; the original's local server keeps one for the whole
  process and lists every log.
- **A shift by 2^31 or more**: the original shifts as the processor does
  (`1 << -3000000000` is 1, by the count's low 5 bits once it is cut to 32
  bits); tflip shifts by the whole count (0), as for every other count of
  the width or more.
- **Long errors are not cut**, but for those that name a failed trigger or
  a recursion over 64 levels (7093), which tflip cuts at 562 characters as
  the original does.
- **`AdsGetVersion`** describes the engine as tflip, its release and
  the target it is built for (`tflip 0.27.0 i686-pc-windows-gnu`, the
  release alone where that would be longer than the original's 42 bytes); `tflip.ini`'s
  `[compat] version = original` gives the original's description (`Advantage
  32-bit Client Engine for Windows`, from its 64-bit DLL too), or any other
  text given there.
- **Diagnostics** go to stderr or `TFLIP_LOG_FILE`, by channel
  (`TFLIP_DEBUG`, in the style of Wine's `WINEDEBUG`): `fixme` for stubs,
  `remote` for connections to servers, and, off by default, `sql` for the
  statements clients run and `err` for every call that fails.
- **No circuit reset after failed logins**: the original's remote server
  answers some failed or locked-out connects by resetting the circuit
  (6624), and every later connect of that client process too; `tflipd`
  always answers 7078 or 7113, which is what the probes record when it
  does not reset.

## Not on purpose

Everything else that differs is a bug or something not done yet, listed in
[KNOWN_ISSUES.md](KNOWN_ISSUES.md). The largest gaps are the cases of
functions that still fail as not implemented (renaming a publication or a
subscription, the rest of deploying a database; see
[KNOWN_ISSUES.md](KNOWN_ISSUES.md#functions-not-implemented)) and
replication that runs.
