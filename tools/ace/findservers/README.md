# AdsFindServers probe

Settles what `AdsFindServers` does and why it returns `50` — a value that is
not an ADS error code — with a NULL handle in a server-less environment. See the
`AdsFindServers` entry in [`docs/ADS_BUGS.md`](../../../docs/ADS_BUGS.md).

- **`findservers.c`** — loads an `ace32.dll`, calls `AdsFindServers` with a few
  option masks, reports `rc` + last error, and on success walks the returned
  cursor dumping every field of every row. An optional second argument is a
  remote `Data Source` for a plain `AdsConnect60`, to tell a broken discovery
  apart from "no server running". Build with `i686-w64-mingw32-gcc`; `run.sh`
  builds and runs it under Wine, or copy the `.exe` to real Windows.

## What the boundary trace found (real Windows 10, native `ace32.dll`)

Process Monitor watching the OS boundary (not the engine's code) showed the
call **IP-multicasts a 50-byte request (`01 2a 00 …`) to `224.0.1.55:2989`**,
four retries, every send `SUCCESS`, then waits for replies. With no server it
returns `50` and writes the output handle to `NULL`. `50` is not an ADS error
code (the SDK defines none with that value) and `AdsGetLastError` is empty, so
the empty-discovery path reports a malformed result — the bug — where the reply
path returns `AE_SUCCESS` with an empty cursor. `findservers.c` reports the
handle on every return code so this is visible.

- **`responder.ps1`** — joins `224.0.1.55:2989` and echoes each request back.
  With it running, every valid `ulOptions` (0, `ADS_FS_MULTICAST_ONLY`, `ADS_FS_CONNECT_ALL`)
  return `rc=0` and hand back the 13-column result cursor (`IP Addr`, `Port`,
  `Multicast Addr`, `Server Name`, `Serial Number`, `Version`, `Owner`,
  `Install Date`, `User Count`, `Server Type`, `Up Time`, `Operations`,
  `More Info`) — empty rows only because an echo is not a valid server record.
  This is the decisive proof that the discovery works and `50` is the
  server-less condition.
- **`mcast-selftest.ps1`** — generic UDP multicast + broadcast send/receive
  check, to show the environment itself carries both fine.

## Against a live server (real table data)

The discovery was confirmed end-to-end against the real 12.0 evaluation server
run under Wine (setup in `LOCAL.md`; the Windows-VM copy would
not serve — a trial dialog interactively, `7045` as a service). Start it
and run this probe under Wine on the same host: it
returns `rc=0` with one populated row per address the server advertises
(`Server Name`, `Serial Number`, `Version`, `User Count`, `Server Type`,
`Up Time`, …). Per the docs, `ADS_FS_MULTICAST_ONLY` leaves the detail columns
blank, and `ADS_FS_CONNECT_ALL` records any failed detail-connect in `More Info`
(`Error 6097` for an unreachable address).

## Reproducing the trace

Capture with Process Monitor (`/BackingFile`, `/RunTime`), run `findservers.exe`
inside the window, convert the `.pml` to CSV (`/OpenLog … /SaveAs …`), and
filter to the probe process. The network rows show the multicast sends; the
responder flips the result.
