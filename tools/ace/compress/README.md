# Communications compression: black-box findings

What the original engine (12.0 SDK `ace32.dll`) does with its compression
options — the `Compression` connect-string option of `AdsConnect101` and the
`ADS_COMPRESS_*` options of `AdsConnect60` — from driving them under Wine
against the local server and the evaluation server
(under Wine, setup in `LOCAL.md`); **no disassembly**.

    tools/ace/compress/run.sh <work folder> <acesdk folder> [remote]

## Findings (2026-10-23)

- **Values.** `Compression=` takes `INTERNET`, `ALWAYS` and `NEVER` in any
  case, with surrounding spaces (`internet `). Anything else — empty, `1`,
  `TRUE`, `ON`, `OFF`, `NONE`, `BOGUS` — fails with 5019
  (`AE_INVALID_OPTION`), the value added to the error's text
  (`Error 5019:  An invalid option was specified.  BOGUS`; nothing after an
  empty one).
- **Checked first, for every server type.** A bad value fails the same way on
  the local server (where the help says the option is ignored), and on a
  remote connect string naming an unreachable server it fails with 5019
  before the server is tried (a good value there fails 6414, connection
  refused).
- **Nothing reports it.** `AdsGetConnectionProperty` answers the same with
  and without compression (properties 0–7; 8 is 5019), for every value.
- **The server's setting** is reported by `AdsMgGetConfigInfo` in
  `ADS_MGMT_CONFIG_PARAMS.usCompressionLevel` (offset 0x358) as an
  `ADS_COMPRESS_*` number: 12 (`ADS_COMPRESS_INTERNET`, the default) on the
  evaluation server, 0 on the local server.
- **`AdsConnect60` options.** `ADS_COMPRESS_ALWAYS` (4), `_NEVER` (8),
  `_INTERNET` (12), `ADS_REPLICATION_CONNECTION` (0x10) and 4 | 0x1000 are all
  accepted on the local server.

Not probed: whether a client that says `ALWAYS` is refused (7084, "a client
requested communications compression, but the server is not configured to
allow it") by a server set to `NEVER`; the help's table says compression is
just off then, and tflip follows the table.
