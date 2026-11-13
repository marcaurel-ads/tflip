# Restructures of tables with full-text orders

Generators of the scenarios `fixtures/write/restructure_fts*`, run with
`tools/ace/write/run.sh <work> <ace32.dll> fixtures/write/<name>` after the
generator has written the scenario's `setup.txt` and `ops.txt`:

| script | scenario |
| --- | --- |
| `types.py <folder> adt` | `restructure_fts_types` (fields added, renamed, dropped, retyped; a large table; options kept) |
| `types.py <folder> cdx` / `vfp` | `restructure_fts_cdx`, `restructure_fts_vfp` |
| `layout.py <folder>` | `restructure_fts_layout` (the pages of large orders after a restructure) |
| `other.py <folder> adt` / `cdx` | `restructure_fts_other`, `restructure_fts_other_cdx` (orders in files of their own) |
| `dict.py <folder> adt` | `restructure_fts_dict` (a dictionary's tables, by SQL and `AdsRestructureTable`) |
| `aes.py <file>` | `tools/ace/crypto/aeswrite/47_restructure_fts.txt` (AES and RC4 dictionaries that encrypt; see `aeswrite.sh`) |

`restructure_fts` itself is written by hand (a free ADT table of each shape).
