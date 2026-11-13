# Platforms

What runs where, and what is specific to Windows or to other platforms.

## Overview

| Crate | Runs on | Platform-specific code |
| --- | --- | --- |
| `tflip-storage` | Anywhere | Byte-range file locks (`fcntl` on Unix, Win32 locks on Windows) and, on Windows, the user locale's collation and path naming (`os.rs`) |
| `tflip-engine` | Anywhere | Small `cfg` branches (file identity in `shared.rs`, syncing the log's folder in `txlog.rs`, the host collation in `system_collation.rs`); it also assumes Windows file system behaviour (see [File system](#file-system)) |
| `tflip-protocol` | Anywhere | None (rustls with the ring provider, which MinGW builds) |
| `tflip-ace` | Built for Windows; its tests also run on the host | The ANSI code page conversion in `ansi.rs`; finding `tflip.ini` next to the DLL in `config.rs` |
| `tflip-aep` | Linux and Windows | Loading a library and finding its functions (`dlopen` on Unix, `LoadLibraryW` on Windows), and finding the copy of tflip's client library a library links |
| `tflipd` | Linux; Windows 10 tried (see [The server](#the-server)) | Where it keeps its certificate (`$XDG_STATE_HOME`, `%LOCALAPPDATA%`), and the key file's mode on Unix |

`cargo test` on a Linux host runs the tests of every crate, the FFI tests of
`tflip-ace` included. They call the exports in the test process; the built
DLLs themselves are not tested by it. Running the probes in `tools/ace/`
with tflip's `ace32.dll` under Wine and comparing their results with the
original's is done by hand, scenario by scenario, and not yet part of the
test suite.

## The client library

`tflip-ace` is the client library. Cargo builds it as `tflip_ace.dll`;
`cargo xtask dist` builds it for 32-bit Windows clients
(`i686-pc-windows-gnu`) as `ace32.dll` and for 64-bit ones
(`x86_64-pc-windows-gnu`) as `ace64.dll`, in `target/dist`.
[`.cargo/config.toml`](../.cargo/config.toml) holds the 32-bit target's linker
flags and the MinGW toolchains both need:

- The exports are `extern "system"` (stdcall on 32-bit Windows) and are
  exported without the `@N` suffix (`--kill-at`), as ACE clients import
  them.
- Off Windows, `extern "system"` is the C calling convention, so the same
  code builds for the host tests.

The 64-bit `ace64.dll` needs no linker flags, as 64-bit Windows has no
stdcall name decoration. Under Wine it answers the probes as the 32-bit DLL
does, and as the original `ace64.dll` does.

The DLL replaces the original one in the client's folder. Under Linux,
Windows clients such as Advantage Data Architect run under Wine.

## Character encoding

| Layer | Text is |
| --- | --- |
| Files (`tflip-storage`) | 8-bit fields, field names, tag names and index expressions: Windows-1252, which maps each of the 256 bytes to a character of its own, so they carry the stored bytes. NChar, NVarChar and NMemo: UTF-16LE. |
| Engine | UTF-8 `String`s (8-bit table text as above); file names and paths are `OsString`/`Path` |
| ACE API (`tflip-ace`) | Table data as its bytes; paths, connect strings and messages in the process's ANSI code page (`CP_ACP`); UTF-16 for the `…W` calls, or UTF-8 where a call asks for it (`ADS_GET_UTF8`) |

As in the original engine (probed under code pages 1252 and 1251), the 8-bit
text of ADT tables is bytes in the client's ANSI code page: `AdsGetField`
returns the bytes the table holds whatever the code page, and what a client
seeks, filters on or names is its bytes. Only where a client asks for
Unicode (`AdsGetFieldW`, `AdsGetStringW`, `ADS_GET_UTF8`) are they read in
its ANSI code page: byte 0xC0 is `À` under code page 1252 and `А` under
1251. The Unicode field types are Unicode, converted to the ANSI code page
for the 8-bit calls, and fail with `AE_UNICODE_CONVERSION` on a character it
lacks.

`tflip_ace::ansi` keeps the two apart: `data_in` and `data_out` for table
data, and `MultiByteToWideChar` and `WideCharToMultiByte` for the rest. Off
Windows it takes the ANSI code page to be Windows-1252, a Windows client's
default, so the host tests see what such a client sees.

The 8-bit text of an OEM table (a Clipper or FoxPro table opened with
`ADS_OEM`) is bytes in the client's OEM code page. The getters convert it as
the original engine does: to the ANSI code page best fit (`░` is `¦`), or to
Unicode. Under code page 437 the original uses tables of its own (0x7F is
U+001A, 0xE6 U+03BC, unlike Windows' code page 437), which `tflip_ace::ansi`
has, probed through it; other OEM code pages go through Windows
(`OemToCharBuffA`, `MultiByteToWideChar`). Off Windows the OEM code page is
taken to be 437.

Known gaps are in [KNOWN_ISSUES.md](KNOWN_ISSUES.md#fields): text given
through the Unicode calls, collations, and text given for OEM tables.

## File system

The engine runs anywhere, but it treats paths and file names the way a
Windows client expects them. That matters for `tflipd` on a Linux server.

- **Separators**: clients use `\`. For the local server, a table name such
  as `sub\orders.adt` is joined onto the connection's path as it is; on
  Linux the backslash becomes part of the file name. `tflipd` splits
  names on `\` and `/`. The table listing accepts both in masks.
- **Case**: Windows file systems ignore case, and clients rely on that
  (`ORDERS.ADT` for `orders.adt`). The table listing matches masks without
  regard to case. `tflipd` finds shares, folders and tables regardless of
  case; the local server on Linux does not, so there such opens fail.
- **Drive letters**: a connection path on a drive that does not exist
  (`Q:\data`) fails with `AE_NO_DRIVE_CONNECTION`, checked only on Windows.
- **Client paths**: a client names its data on a server by share
  (`\\server\share\data`), which `tflipd` maps to its own folders
  (`--share NAME=DIR`); see [`tflip_engine::shares`](../crates/tflip-engine/src/shares.rs).
  A path through a mapped drive (`X:\data` for `\\server\share\data`)
  reaches the share's server too, as on Windows the client names a
  connection's path by its UNC path (`tools/ace/mapped`).

Field and order names are matched without regard to case on every
platform; that is ACE's behaviour, not the file system's.

## The server

`tflipd` runs on Linux and serves Windows clients that load tflip's DLL,
under Wine as well. Text travels decoded, so each client converts it to its
own ANSI code page, as for the local server; the gaps under
[Character encoding](#character-encoding) apply the same way. It builds for
Windows and has served a client on Windows 10 (`tools/ace/mapped`); its
tests pass there, as the workspace's do, but it has not been run as a
service. Neither has it run an external procedure's library on Windows
(a DLL linking `ace64.dll`, found as the module of that name); on Linux it
runs shared objects linking `libtflip_ace.so`. See the remote server
in [TODO.md](TODO.md).
