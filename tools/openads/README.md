# OpenADS cross-tests

[OpenADS](https://github.com/FiveTechSoft/OpenADS) (Apache 2.0, C++) is
another open replacement for the Advantage Client Engine: a drop-in
`ace32.dll`/`ace64.dll`/`libace.so`, with a server and wire protocol of its
own. Both implement the same API, so each can run the other's tests. This
folder holds those runs.

## OpenADS's tests on tflip: `run.sh`

    tools/openads/run.sh [--sap] <OpenADS checkout> <work folder>

`run.sh` builds `libtflip_ace.so`. It then builds the OpenADS unit tests
listed in [`tests.txt`](tests.txt) against that library and runs each file
in its own process. The comment at the top of `run.sh` says what it writes
and how it edits the tests where OpenADS's expectations differ from the
original engine's.

The edits are needed because of these differences:

- **`ADSHANDLE`**: OpenADS's `ace.h` makes it 32 bits on every platform.
  `ace.h` makes it 64 bits under `x64`. Without the edit, tflip's handles
  overwrite the tests' stack and heap.
- **Constants**: OpenADS gives 70 of the 232 constants it shares with
  `ace.h` other values. Examples: `ADS_IGNOREFILTERS` 3 (2),
  `ADS_HARDSEEK` 0 (2), `ADS_STRINGKEY` 1 (2), `ADS_COMPATIBLE_LOCKING` 2
  (0), `ADS_CISTRING` 25 (20). 15 error codes differ too, among them
  `AE_LOCK_FAILED` 5013 (5035) and `AE_INVALID_HANDLE` 5024 (5018).
- **Literal zeros** where `ace.h` defines no 0: filter options, seek and
  key data types, the AOF resolve option, character types and rights
  options. The original rejects them (`fixtures/errors/record_count.json`,
  `fixtures/errors/table_options.json`), so a test would stop at its
  setup. [`zeros.cpp`](zeros.cpp), linked in with `--wrap`, passes `ace.h`'s
  value with the meaning OpenADS gives 0 instead.
- **Unquoted `name.dbf` in SQL** and **a statement's default table type**:
  what the original does is recorded in `fixtures/sql/links.json`. The
  default table type is `ADS_ADT` (see the comment in `run.sh`).

### Against the original: `--sap`

With `--sap`, `run.sh` also builds the same tests, with the same edits and
wrappers, for Windows (MinGW-w64), links them against the original's
`ace64.dll` and runs them under Wine. [`compare.py`](compare.py) then
lists, per file, the checks that fail on one engine only, or on both with
other values, in `compare.txt`. That settles most failures without a probe
of their own:

- a check that fails on both with the same values is the test's
  expectation, not a tflip bug;
- a check that fails on tflip only is a tflip bug, or the original stopped
  at an earlier `REQUIRE` of the same test case;
- a check that fails on the original only is where tflip agrees with
  OpenADS but not the original, or tflip stopped earlier.

Run of 2026-11-14, OpenADS v1.09.75 (`1ef469a`), tflip 0.26.0 with the
option checks of `fixtures/errors/table_options.json`, x86_64 Linux and the
original's `ace64.dll` 12.0.0.2 under Wine. The 91 files hold 258 test
cases:

| | tflip | the original |
| --- | --- | --- |
| test cases passed | 89 | 41 |
| files passed | 13 | 13 |
| files failed | 77 | 78 |
| files timed out (120 s) | 1 | 0 |

The original passes far fewer, so most of OpenADS's expectations are not
the original's. Of the failing checks, 108 fail on both with the same
values, 18 on both with other values, 91 on tflip only and 132 on the
original only. The file that timed out, `abi_cdx_expr_index_scale_test`,
took about 8 minutes, as tflip counted the visible records for each
`AdsGetRecordNum(…, ADS_RESPECTFILTERS)`; it takes under a second since an
exclusive handle keeps which records are hidden, and fails its check as the
original does.

A failure counts as a tflip bug only once the original passes the check.
The original resolves a relative table name against the current directory,
where tflip takes the connection's folder. The tests make their tables in
temporary folders of their own and then name them relatively (`data`), so
on the original they opened whatever another test had left under that name
in the shared working folder: `abi_custom_key_test`'s order on its field
`TAG` failed with 3011 because the `data.dbf` it opened had only a field
`ID`. [`sapcwd.cpp`](sapcwd.cpp), linked into the original's build only,
makes the folder of each successful `AdsConnect60` the current directory.
Since then the original refuses the tests' hand-made DBF files with 7016:
their headers date the last update 00/00/00, and the original opens no DBF
table whose month is not 1 to 12 or whose day is not 1 to 31
(`fixtures/dbf/header_dates.json`). tflip now does the same, and `run.sh`
dates those headers 2024-01-01 in both builds, so that the tests run past
their setup.

Before `sapcwd.cpp`, the original passing was not always the original
agreeing: a test that names its table relatively and its index files by
full path, as
`abi_adi_ordinal_stable_test` does, has them in two folders on the
original, where `ord.adi` is then not the table's auto-open index and
`AdsOpenIndex` of it succeeds. With both in one folder the original gives
5065, as tflip does (`fixtures/errors/index_files.json`).
Each one becomes a probe and a fixture of tflip's own before it is fixed,
as `fixtures/errors/table_options.json` was.

## tflip's probes on OpenADS: `probes.sh`

    tools/openads/probes.sh <DLL, a Unix path> <work folder> [<probe>...]

`probes.sh` runs the probes in [`probes.txt`](probes.txt) under Wine
against the given DLL and against the original's `ace64.dll`, and compares
the two. The comment at the top of `probes.sh` explains the output table.

The probes are built for **x86_64**. OpenADS's 32-bit DLL exports its
functions as cdecl under their plain names, even in the MSVC build (its
`src/CMakeLists.txt`, `abi/ace_stdcall_x86.c`). The original exports
stdcall under those names, and the probes call it that way. On x64 there
is one calling convention, so the comparison is fair. A compiler shim
builds the unchanged probe scripts for x86_64 with
[`probe64.h`](probe64.h) force-included. The 64-bit original matches
the golden, which comes from `ace32.dll`, except for a few lines. Those
are counted in the table's `x64` column, and `bookmarks` has the most
because bookmarks are longer there.

Building OpenADS's DLL with MinGW, statically linked so that it loads
from any folder:

    cmake -S <OpenADS> -B <build> -G Ninja -DCMAKE_SYSTEM_NAME=Windows \
        -DCMAKE_C_COMPILER=x86_64-w64-mingw32-gcc \
        -DCMAKE_CXX_COMPILER=x86_64-w64-mingw32-g++ \
        -DCMAKE_RC_COMPILER=x86_64-w64-mingw32-windres \
        -DCMAKE_BUILD_TYPE=Release -DOPENADS_BUILD_TESTS=OFF \
        -DOPENADS_WARNINGS_AS_ERRORS=OFF -DOPENADS_WITH_HTTP=OFF \
        -DOPENADS_WITH_SQLITE=OFF -DOPENADS_BUILD_ACE_STATIC=OFF \
        -DCMAKE_SHARED_LINKER_FLAGS=-static
    cmake --build <build> --target openads_ace   # <build>/src/libopenace64.dll

First run (2026-11-13), OpenADS v1.09.75. "Same" counts the lines of the
original's answers that OpenADS gives too:

| Probe | Same | Probe | Same |
| --- | --- | --- | --- |
| `fields` | 855 / 5207 | `sql_statements` | 76 / 1634 |
| `dbf_getters` | 4684 / 12345 | `sql_select` | 382 / 2097 |
| `dbf_indexes` | 2470 / 8691 | `sql_dml` | 51 / 439 |
| `bookmarks` | 53 / 240 | `sql_dbf` | 49 / 501 |
| `keytype` | 3 / 11 | `sql_orderby` | 75 / 1362 |
| `cursor_set` | 2 / 317 | `sql_groupconcat` | 32 / 123 |
| `param_n` | 0 / 2242 | `sql_derived` | 21 / 1338 |
| `dd`, `dd_rules`, `dd_login`, `dd_ri` | 2–5 each | other `sql_*` (9) | 3–34 each |

What stands out:

- **49 functions the probes use are not exported.** Among them are
  `AdsGetShort`, `AdsGetMoney`, `AdsGetTime`, `AdsIsFieldBinary`,
  `AdsGetBookmarkLength`, `AdsGotoBookmark`, `AdsCompareBookmarks`,
  `AdsLocate`, `AdsGetSQLStatement`, `AdsGetTransactionCount`,
  `AdsSetStringFromCodePage`, `AdsIsIndexCompound`/`FTS`/`PrimaryKey`,
  `AdsGetIndexCollation`, and the dictionary's replication, user-group and
  deploy calls. In all, OpenADS exports 440 `Ads*` names, and `ace.h` has
  465.
- **Typed getters on the wrong field type**: in 1,952 of the `fields`
  cases, the original refuses with 5066 (`AE_INVALID_FIELD_TYPE`) and
  OpenADS converts and returns 0. Another 1,166 cases hit missing
  functions.
- **Dictionaries the original made do not open**: `AdsConnect60` on
  `dict.add` answers 5174, and every login 7077. `param_n` needs a
  dictionary connection to start, so it prints nothing, and the `dd`
  probes stop after their connects.
- **`AdsGetKeyType`** reports 4 with key length 0 for every order.
- **Writes through a live SQL cursor** fail with 5000, "table opened
  read-only".
- **SQL cursors**: `COUNT(*)` is a numeric of 11 (the original: an integer
  of 4), the handle type is a table's (1, not 5), the rows affected by a
  `SELECT` are 0 (not `0xFFFFFFFF`), and the temporary files have other
  names.
- **Error codes**: OpenADS answers 5004 for "function not available". In
  `ace.h`, 5004 is `AE_FILE_NOT_FOUND`, and `ace.h` has no
  "function not available" code.

Each difference here is between OpenADS and the original, so these are
OpenADS's to settle. tflip's own probe results are unchanged.
