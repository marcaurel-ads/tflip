# Client configuration: black-box findings

What black-box probing established about the files the Advantage Client Engine
(`ace32.dll`, ACE 12.0 SDK) reads when a client connects — in particular
`ads.ini`. Everything here comes from driving the documented ACE API and
watching the engine's file access under Wine (`WINEDEBUG=+file`); **no
disassembly**.

## `ads.ini` is not acted on by ACE (2026-10-20)

`ace32.dll` **opens** `ads.ini` during a connect — it searches for it by the
Windows `SearchPath` order (the DLL/executable directory, the current
directory, the system and Windows directories) — but it does **not** resolve
an `ads.ini` `[Databases]` alias into the connection's path. A connect name is
always taken as a folder path relative to the current directory:

```
$ adsini.exe ace32.dll AllTypesRemote 3     # with an ads.ini that has
conn("AllTypesRemote", types=3) rc=0  path(rc=0)=".\AllTypesRemote"
#   [Databases]
#   AllTypesRemote=\\127.0.0.1:6262\fixtures\all_types;A
```

The resolved path is `.\AllTypesRemote`, not the aliased
`\\127.0.0.1:6262\fixtures\all_types`. The same holds for a local alias and
for every server-type combination (`AdsGetConnectionPath` reports the folder a
local connection resolved to; a remote-only connect to an unreachable alias
fails `6420`).

Alias resolution lives one layer up, in the **TDataset/ARC Delphi** client
(`adscnnct.pas` `GetAliasInfo`, `adsdata.pas` `GetAliasPathAndFileName`): it
reads `[Databases]` (`alias = path;TYPE`, `TYPE` one of `A`/`C`/`N`/`V`) and
the per-alias `[<alias>_info]` section (ARC UI state — `ServerType`,
`Username`, `ExpandedNodes`, `BackupDestination`, …), then calls
`AdsConnect101` with an **already-resolved** path. So a program that does not
use TDataset never gets `ads.ini` alias resolution from ACE.

**Consequence for tflip.** tflip's `ace32.dll` faithfully does not act on
`ads.ini` either; its own configuration (server fingerprints, transactions,
connection timeouts) lives in `tflip.ini` (see `tflip_ace::config`). Resolving
`ads.ini` aliases would be a *deliberate divergence* from the engine, not
parity, and was decided against.

## Probe

`adsini.c` — `adsini.exe <ace32.dll> <alias-or-path> <server-types>`; run from
a directory holding an `ads.ini`. Prints what `AdsGetConnectionPath` reports
for the connection, which shows whether the alias was substituted.

# `AdsConnect101`'s connect string options

`connect101.c` (run by `run_connect101.sh`, which records
`fixtures/config/connect101.json` and `connect101_values.json`) connects with
`Data Source=<folder>;ServerType=LOCAL;<options>` and prints what the
connection and `AdsOpenTable101` do. `connect101.txt` lists the option sets
tried with a table of each type; `connect101_values.py` writes
`connect101_values.txt`, the values tried for each key, with no tables.

What it showed (also in `crates/tflip-ace/src/connect_options.rs`):

- The keys (in any case, `Data Source` also as `DataSource`, `User ID` as
  `UserID`) are `Data Source`, `ServerType`, `User ID`, `Password`,
  `Compression`, `CommType`, `TLSCiphers`, `TLSCertificate`, `TLSCommonName`,
  `FIPS`, `EncryptionPassword`, `EncryptionType`, `DDPassword`, `TableType`,
  `CharType`, `ReadOnly`, `Shared`, `LockMode`, `SecurityMode`, `ShowDeleted`,
  `TrimTrailingSpaces`, `Epoch`, `Exact`, `DateFormat`, `Decimals`,
  `SQLTimeout`, `Collation`, `CheckFreeTableAccess`, `IncrementUserCount`,
  `Pooling`, `Max Pool Size`, `Connection Lifetime` and `Connect Timeout`. Any
  other key (`Server`, `Server Type`, `UID`, `MaxPoolSize`, ...), found before
  any value is checked, fails with 5019 and the key added to the text. So does
  a value that is empty (the key is added; for `Compression` and `CommType`
  nothing) or wrong (the value is added: in upper case for the names of
  `TableType`, `CharType`, `LockMode` and `SecurityMode`). `Data Source` empty
  or missing fails with 5019, "Data Source must be provided in connection
  string.".
- Values: the booleans (`ReadOnly`, `Shared`, `ShowDeleted`, `Exact`,
  `CheckFreeTableAccess`, `IncrementUserCount`, `Pooling`) take `TRUE` and
  `FALSE` in any case, `TrimTrailingSpaces` and `FIPS` also `1`, `0`, `YES`
  and `NO`; `TableType` takes `ADT`, `CDX`, `NTX` and `VFP`, `CharType` `ANSI`
  and `OEM`, `LockMode` `COMPATIBLE` and `PROPRIETARY`, `SecurityMode`
  `CHECKRIGHTS` and `IGNORERIGHTS` (each also with the `ADS_` name of the
  constant), `CommType` `TCP_IP`, `UDP_IP`, `IPX` and `TLS`, `EncryptionType`
  `RC4`, `AES128` and `AES256`; `Epoch`, `Decimals` and `SQLTimeout` any text
  that starts with a digit; `DateFormat` what `AdsSetDateFormat` takes;
  `Collation` a name of the repository (`ansi` too). The pool and timeout keys,
  `ServerType` and the secrets take anything.
- `AdsOpenTable101` opens every table as `TableType` says (ADT without it),
  with `CharType` (an ADT table is ANSI), read-only (`ReadOnly`, open options
  `0x10012`) and exclusively (`Shared=FALSE`, `0x100f1`); `LockMode=COMPATIBLE`
  gives a DBF table compatible locking (an ADT table stays proprietary).
  `DateFormat`, `Exact` and `Collation` are the connection's own
  (`AdsGetDateFormat60`, `AdsGetExact22`, `AdsGetCollation`). `TrimTrailingSpaces`
  trims what `AdsGetString` reads of text fields; `ShowDeleted`, `Epoch`,
  `Decimals`, `SecurityMode` and the rest show no effect in these probes.
- The options handle `phConnectionOptions` gets is a nonzero handle, which
  `AdsGetHandleType` calls a table (2).
- Without the OpenSSL libraries (`FIPS=TRUE`, the TLS keys with a TLS
  transport) the original fails with 5214.
