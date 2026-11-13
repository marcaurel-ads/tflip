# Paths through mapped drives: black-box findings

What the original engine (12.0 SDK `ace32.dll`) makes of a connection's path,
in particular one through a drive mapped to a share, run natively on the
Windows 10 VM (10.0.19045, see `LOCAL.md`); **no disassembly**. Wine cannot
answer this: it has no mapped network drives, and there the original reports
a relative path as `.\name` (`tools/ace/config`), which Windows does not.

    i686-w64-mingw32-gcc -O1 -Wall -o mapped.exe mapped.c
    mapped.exe <ace32.dll> <path> [<table>]

`mapped.exe` connects with `AdsConnect60` for each server-type mask (1 local,
2 remote, 3 both) and prints `AdsGetConnectionType`, `AdsGetServerName` and
`AdsGetConnectionPath`, and opens `<table>` if given. `paths.ps1` runs it over
the paths below for the original's DLL and tflip's; `tflipd.ps1` runs it
through a mapped drive whose share `tflipd` serves.

## Findings (2026-10-25)

- **A connection's path is made full.** `rel`, `.\rel` and `C:rel` (with the
  current folder `C:\work`) are `C:\work\rel`; `C:` is `C:\work`; `..` and `.`
  are resolved, `/` becomes `\`, and the case is kept (`C:\WORK\Rel`). A
  folder that does not exist is no error (`C:\nope`).
- **Through a mapped drive it is the share's UNC path**, also for the local
  server: with `Y:` mapped to `\\host.lan\Data`, `Y:\mapped`, `Y:mapped` with
  the current folder `Y:\`, and `y:/mapped/sub/` are `\\host.lan\Data\mapped`
  (`…\sub`), and `AdsGetServerName` is `host.lan`, not `Y:`. A remote-only
  connect asks the share's host for a server (6420 when it has none); a
  dictionary's error names it by its UNC path too
  (`… (\\host.lan\Data\mapped\sub\dictionary.add) is not found on the server.`).
- **No trailing separator**, not even on a root: `C:\work\rel\` is
  `C:\work\rel`, `C:\` is `C:`, `Y:\` is `\\host.lan\Data`,
  `\\host.lan\Data\mapped\` is `\\host.lan\Data\mapped`. A UNC path is
  otherwise kept as given (`\\127.0.0.1\C$\work`).
- **A table named by a full path through the drive opens** on such a
  connection (`Y:\mapped\sub\all_types.adt`), as by its UNC path.

## In tflip

`tflip_storage::universal_path` names a client's connection path this way on
Windows (`GetFullPathNameW`, then `WNetGetUniversalNameW`); a connection
through a mapped drive with a remote server type goes to `tflipd` on the
share's host, and a table or connection named through the drive is matched by
its UNC path. A drive's root keeps its separator (`C:\`): `C:` alone names
that drive's current folder. `tflipd.ps1` checked the route end to end on the
VM: `X:\sub`, with `X:` mapped to the VM's own share, connects to `tflipd`
(type 2) and opens `X:\sub\all_types.adt` and `all_types.adt` there.
