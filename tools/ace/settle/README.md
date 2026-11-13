# Settling tests (real-Windows oracle)

Scripts that settle the `[unconfirmed]` entries in
[`docs/ADS_BUGS.md`](../../../docs/ADS_BUGS.md) — suspected engine defects that
could not be isolated under Wine. Each is a command script for the
[`tools/ace/write`](../write) driver (`write.c`), so no new probe binary is
needed:

```
write.exe <ace32.dll> <empty data folder> settle/<name>.txt  > out.json
```

They build their own tables, so they need only an empty, writable data folder.

## Running against the native engine

The oracle is a native `ace32.dll` on real Windows (see
[`CONTRIBUTING.md`](../../../docs/CONTRIBUTING.md) and the uncommitted
`LOCAL.md` for the Windows VM). Cross-compile the driver and copy it plus the
scripts into the guest:

```sh
i686-w64-mingw32-gcc -O1 -Wall -o write.exe tools/ace/write/write.c
# ...copy write.exe and settle/*.txt into the guest, beside a local copy of the
#    acesdk folder (ace32.dll + adsloc32.dll + aicu32.dll + ICU/collation data),
#    then, with the working directory in that folder:
write.exe ace32.dll C:\data\empty settle\gotopast.txt > gotopast.json
```

Loading `ace32.dll` by a UNC path (`\\host.lan\Data\...`) faults on real
Windows; copy the self-contained acesdk folder local and run from inside it.

## What they settled (real Windows 10, 10.0.19045, native `ace32.dll` v12.00)

- **`ntxneg.txt`** — Negative numbers **are** found in an NTX order. An NTX
  numeric order sorts and seeks negatives numerically, byte-for-byte like a CDX
  order on the same data. No defect (`[by-design]`).
- **`gotopast.txt`** — A past-the-end `AdsGotoRecord` returns `5022`
  (`AE_INVALID_RECORD_NUMBER`) **iff the current record is a deleted record**;
  otherwise it returns `0`. Either way the cursor lands at EOF. A deleted record
  elsewhere is irrelevant (`[quirk]`).
- **`twoparents.txt`** — A child can be the child of two relations at once; it
  follows whichever parent moved most recently — last mover wins (`[by-design]`).

All three answers are identical under Wine and on native Windows.
