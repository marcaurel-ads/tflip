# AdsSetCollation name check

`setcollation.c` records which collation names `AdsSetCollation` accepts, for
the 7147 refusal tflip implements (`crates/tflip-ace/src/settings.rs`,
`check_collation`, and `tflip_storage::collation::Dynamic::canonical`).

    i686-w64-mingw32-gcc -O1 -Wall -o setcollation.exe setcollation.c
    setcollation.exe <ace32.dll> <data folder>

Run it with the engine's collation files (`ansi.chr`, `adscollate.adt`) beside
the DLL -- on the native VM (see `LOCAL.md`), from `C:\acesdk`. The findings are
in the file's header and in `docs/KNOWN_ISSUES.md`. The statement-level check
(`AdsStmtSetTableCollation`) behaves the same and is probed through
`tools/ace/sql/sql.c`'s `collation` command.

`setcollation_dd.c` runs a list of names through `AdsSetCollation` and
`AdsStmtSetTableCollation` on a free-table and a dictionary connection (with a
server path, to a remote server) and prints the codes and error texts; its
findings are in `docs/ADS_BUGS.md` and `Dynamic::is_icu_locale`.

# The static collation of an empty ANSI_CHAR_SET

`sysdefault.c` records which static collations the engine uses with
`ANSI_CHAR_SET` and `OEM_CHAR_SET` empty: the NTX order of a DBF holding each
byte 1..255, opened ANSI and OEM, and of an ADT table the ADI key of each byte,
`UPPER`/`LOWER`, and the collation CRC the ADI records. `sysdefault.py` runs it
under Wine once per locale (Wine turns `LC_ALL` into the Windows locale) and
writes `fixtures/collation/sysdefault.json`:

    WINEPREFIX=<a scratch prefix> tools/ace/collation/sysdefault.py <acesdk folder> <work folder> \
        en_US.UTF-8 de_DE.UTF-8 sv_SE.UTF-8 ... > fixtures/collation/sysdefault.json

On Windows run `sysdefault.exe <ace32.dll> <empty folder>` from a folder whose
`adslocal.cfg` leaves both settings empty; set the user locale with
`Set-Culture` and the system one with `Set-WinSystemLocale` (and a reboot).
What it showed is in `crates/tflip-engine/src/system_collation.rs`: under US,
UK and Canadian English (by the language abbreviation `ENU`/`ENG`/`ENC`) the
bytes weigh themselves; else each weighs its rank in `CompareStringA`'s order
under the user locale.

`locales.c` (no Advantage code) lists that collation for every locale Windows
knows, and `gen_locales.py` turns its output into
`crates/tflip-engine/src/system_collations.bin`, which tflip uses where it
cannot ask Windows:

    wine locales.exe > locales.txt
    tools/ace/collation/gen_locales.py locales.txt > crates/tflip-engine/src/system_collations.bin

# Collation mismatch options

`mismatch.c` (run by `run_mismatch.sh`, which also builds the custom-order
table with `mismatch_custom.c` and records `fixtures/collation/mismatch.json`)
opens a table whose ADI index was built under another collation than the
connection's with `ADS_REINDEX_ON_COLLATION_MISMATCH` and
`ADS_IGNORE_COLLATION_MISMATCH`. What it showed:

- Without either, the open fails with 5175 for an index of another static
  collation (the CRC the ADI records differs), and with 5209 ("Table
  collation: ..., Index collation: ...", `(none)` for an empty one) for another
  dynamic collation, which includes a table with a collation and an index
  with none.
- `ADS_IGNORE_COLLATION_MISMATCH` (0x200) opens the table anyway, the
  index file untouched; text sought in it is weighed by the connection's
  collation, not the index's (so a German-built index under the default
  collation finds `ä` among the keys of `a`).
- `ADS_REINDEX_ON_COLLATION_MISMATCH` (0x100) rebuilds the ADI file under
  the connection's collation, as `AdsReindex` does, with the CRC and the
  keys' padding of the new one, whatever the open mode (shared, exclusive,
  read-only). It fails with 5175 and "Cannot open index exclusively for
  auto-reindex." if another connection has the table open, and with 5175 and
  "Index file contains a custom index.  Cannot auto-reindex." for an index
  file with a custom order. Both options fail with 5019 and "Parameter 8 -
  cannot specify both ADS_REINDEX_ON_COLLATION_MISMATCH and
  ADS_IGNORE_COLLATION_MISMATCH". For a dynamic collation the reindex option
  changes nothing: the open fails with 5209.
