# Fixtures

Tables, memo files, indexes and data dictionaries written by the official
Advantage Client Engine 12.0.0.2 (Local Server), with known contents. They are
the ground truth for tflip's readers and writers: each `<table>.json` records
what the engine reports about a table (fields, indexes, every record's raw
bytes, and every value as the engine decodes it).

Each fixture is the result of one SQL script in
[`tools/ace/fixtures/`](../tools/ace/fixtures/), run against the engine once
per table type listed on its `-- types:` line. The generator itself is not part
of this repository, since it needs the proprietary engine. The data in the
scripts is made up for the tests.

## Layout

`fixtures/<table type>/<fixture>/`, where the table type is `adt` (Advantage
proprietary), `ntx` (Clipper), `cdx` (FoxPro) or `vfp` (Visual FoxPro), and
the fixture is the script name.

| Fixture | Types | What it covers |
| --- | --- | --- |
| `all_types` | adt | every ADT field type except ModTime, boundary values, NULLs, ANSI and Unicode text, a deleted record whose slot is reused |
| `freelist` | adt | the deleted-record free list: a table left with a three-link LIFO chain, and one where two inserts reuse the freed slots in place |
| `memo_blocks` | adt | memo, binary and NMemo values from empty to multi-block, with block sizes 8, 256 (default) and 1024; values grown and shrunk by updates |
| `memo_compressed` | adt | compressed Memo, Binary and NMemo fields (16-byte record entries) |
| `memo_alloc` | adt | the `.adm` allocator: a full 32-block slab, a slab released by deleting its memos, extents rounded to their size class and split into runs and fragments, a freed extent on its class list, and a large extent appended and trimmed at the end |
| `indexes` | adt | ADI orders: primary key, ascending, descending, compound, unique case-insensitive; a table with 8 KB index pages; a deleted record |
| `modtime` | adt | ModTime and RowVersion (ModTime values are time-dependent) |
| `dictionary` | adt | data dictionary (`.add`/`.am`/`.ai`) with bound tables, constraints, defaults, an index in a separate file, a view, function, procedure, trigger, user, group and grant |
| `dictionary_props` | adt | a dictionary with distinctive database, table, field, index, view, procedure, function, package, trigger, relation, link, user and permission properties; a view that overflows into the memo file; a web-delta table; a dropped table |
| `psm` | adt | a dictionary with tables `items` and `audit`, stored procedures (`add_item`, `numbers`, `pick`, `external_one`), functions (`sq`, `pad`), the package `util` and five triggers on `items`, which the SQL script and dictionary probes copy |
| `dictionary_encrypted` | adt | default-scheme ("RC4") encrypted dictionary with an encrypted and an unencrypted table and encrypted indexes, and an encrypted table with two memos per record, a multi-block memo and an updated memo |
| `dictionary_ri` | adt | a dictionary for referential-integrity enforcement: a parent table and four children, one per rule (RESTRICT, CASCADE, SET NULL, SET DEFAULT); built and probed by `tools/ace/dd/ri.c` (see `../dictionary/ri.json`) |
| `dictionary_constraints` | adt | a dictionary for its own rules on write: field defaults (`NOW()`, `TODAY()` among them), NOT NULL, minimum and maximum with and without field messages, record validation with and without a message, rows that broke constraints set after them, and BEFORE, AFTER and INSTEAD OF triggers (one raising); built and probed by `tools/ace/dd/constraints.c` (see `../dictionary/constraints.json`) |
| `dictionary_rules` | adt | a dictionary for the order of its rules on write: a RESTRICT, a CASCADE, a SET NULL and a SET DEFAULT rule whose tables have constraints and BEFORE triggers, defaults, minimums and maximums that are expressions on fields of each type, and a table of every type with no rules for `AdsDDSetFieldProperty` to refuse; built by `tools/ace/dd/rules.sh` (see `../dictionary/rules.json`) |
| `expr_keys` | adt | index orders on expressions with non-character result types (`n + 1`, `d * 2`, `dt + 1`, `n > 0`, `UPPER(c)`), to check their key type and seek; built and probed by `tools/ace/expr/keytype.c` (see `../expr/keytype.json`) |
| `adi_keys` | adt | one ADI order per indexable field type, with boundary, empty and NULL values; expression and multi-field keys |
| `adi_tree` | adt | 3,000-record tables with multi-level ADI trees, grown by page splits and then modified, or built in bulk; 512, 1,024 and 8,192-byte pages |
| `adi_options` | adt | conditional, unique, candidate, custom, binary (bitmap, one and two pages), full-text and dynamic-collation orders |
| `dbf_keys` | ntx, cdx, vfp | one index per DBF field type with blank, negative and boundary values, descending and multi-field keys, a deleted record |
| `cdx_options` | cdx, vfp | CDX expressions, `FOR`, xBase `UNIQUE`, binary orders, a second `.cdx`, an `.idx`, 3,000-record tables (grown and built) |
| `ntx_options` | ntx | NTX expressions, `FOR`, xBase `UNIQUE`, descending, a 3,000-record table |
| `ntx_expr` | ntx | computed keys: numeric, date and logical expression results (`a+b`, `i*2`, `ROUND()`, `VAL()`, `RECNO()`, `YEAR()`, `d+1`, `a=b`); the key width is measured on record 1; `e_mul`, `e_ai` and `e_valw` are kept undumped, since navigating them fails with 4004 on rows whose value has a different width |
| `expr_widths` | cdx, adt | the same expressions as CDX and ADI orders: numeric and date results are 8-byte keys; on ADT, `ROUND()` of a Numeric field keeps that field's key |
| `vfp_keys` | vfp | VFP key types with NULL values, a candidate order, a dynamic collation |
| `memo_fpt_blocks` | cdx, vfp | FPT memo files created with `MEMOBLOCKSIZE` 1, 2, 31, 32, 33 and 1000: the block size at byte 6 is *n* × 512 for *n* ≤ 32 in both table types |
| `dbf_ext` | ntx, cdx | the Advantage extended DBF types (Double, Integer, LongInt, GUID, VarChar, NChar, NVarChar, NMemo, Binary, and ShortDate and Image through `AdsCreateTable`), memos from empty to several blocks, grown and shrunk |
| `vfp_more` | vfp | the remaining VFP types (Unicode, LongInt, GUID, `NOCPTRANS` memo) and VarChar/VarBinary lengths for `_NullFlags` |
| `charset_default` | adt, ntx, cdx | ANSI and OEM tables with every character 32–255 and some words, indexed, under the default static collations |
| `charset_german` | adt, ntx, cdx | the same with `ANSI_CHAR_SET=German` and `OEM_CHAR_SET=GERMAN` (`-- config:` lines) |
| `collation_dynamic` | adt, vfp | the same under five dynamic collations (GENERAL, GERMAN, MACHINE, UNIQWT, CZECH) |
| `dbf_types` | ntx, cdx, vfp | the standard DBF types in each flavour, with memo files and a deleted record |
| `cdx_types` | cdx | extended CDX types and a CDX compound index |
| `ntx_index` | ntx | an NTX index file |
| `vfp_types` | vfp | Visual FoxPro types including NULL-able columns |

## JSON

```text
{ "table_type", "record_count", "record_length", "memo_block_size",
  "fields":  [{ "name", "type", "type_name", "length", "decimals" }],
  "indexes": [{ "name", "file", "expression", "condition", "collation",
               "key_length", "key_type", "unique", "descending", "candidate",
               "custom", "compound", "primary_key", "nullable", "fts", "key_count",
               "keys": [{ "recno", "key": "<hex>" }] }],
  "records": [{ "recno", "deleted", "raw": "<record image hex>",
                "values": { "<field>": <value> } }] }
```

Each index order lists its keys in index order, as `AdsExtractKey` returns them
(`null` for full-text orders, which cannot be walked). Index files that do not
open with the table, and tables that need a collation to open, are named on a
`-- dump: <table> <adsdump options>` line in the script (`-x <index file>`,
`-c <collation>`). A `-- chartype: ansi|oem|<collation>` line sets the character type or
collation of the tables that follow, and `-- config: KEY=VALUE` lines change
`adslocal.cfg` for one script (adsdump records `char_type`, `collation`,
`ansi_char_set` and `oem_char_set`). Tables with field types that SQL cannot create (ShortDate,
Image) are made by a `-- create-table: <name> <field definitions>` line, which
calls `AdsCreateTable`.

Values are `{"null": true}`, `{"hex": "…"}` for binary data, or `{"text": "…"}`.
Text is what the engine returns (with padding); 8-bit strings are ANSI
(Windows-1252) bytes written as U+0000–U+00FF. Integers and Money are exact,
Double and CurDouble use `%.17g`, and dates and times add `"julian"` (day
number) and `"ms"` (milliseconds since midnight). `type` is the ACE field type
constant (`ADS_*` in `ace.h`).

For every data dictionary a fixture connects to, `<dict>.system.json` holds
the dictionary's `system.*` tables (`system.tables`, `system.columns`, …) as
`{"system.<name>": <dump>}`, where each dump has the format above without
`raw` record images (`adsdump -q "SELECT * FROM system.<name>" <dict.add>`).

Records are listed in physical order, deleted ones included where the format
keeps them (`AdsShowDeleted`). Dictionary-bound tables are dumped through a
dictionary connection.

## SQL results

`fixtures/sql/*.json` hold results of single SQL expressions and statements
(`SELECT <expr> FROM system.iota` and similar), recorded as
`{"<expr>": {"type", "text"|"hex"|"null"|"error"}}`. Values that depend on the
clock, connection or randomness are recorded but should only be compared by
type.

`fixtures/sql/statements.json` is what the original engine does with the
statements in [`tools/ace/sql/statements.txt`](../tools/ace/sql/statements.txt),
recorded by `sql.c` under Wine (`run.sh` copies the `all_types`, `adi_keys`
and `indexes` fixtures into a fresh free-table folder first, and the
`dictionary_props` and `dictionary` dictionaries into `props/` and `dict/`):
per command, the
return code, `AdsGetLastError`'s text, the rows affected and the cursor in
full (file, open options, fields, every record's `AdsGetField` text). The
folder appears as `<data>`. It covers the statement lifecycle, the system
procedures ARC calls, live and static `SELECT`s, parameters,
`AdsGetSQLStmtParams`, single expressions (`expr`, a `SELECT` from
`system.iota`) and, after a `connect` to a dictionary, its `system.*`
tables.

`fixtures/sql/claims.json` and `claims_huge.json` are the same for
[`tools/ace/sql/claims.txt`](../tools/ace/sql/claims.txt) and `claims_huge.txt`
(`run.sh <work> <dll> claims.txt`, in an empty folder): 944 edge cases of the
grammar, typing, optimizer and execution, one `## <name>` block each, which
rebuild the tables they need (and drop what they made) so that every block runs
on a clean state; `claims_huge.txt` is the seven that need a table of 384 000
rows (its replay is `#[ignore]`d: `cargo test --release -p tflip-ace --
--ignored claims`). The statements come from an earlier recorder, which recorded their answers
first (`claims_cases.json`, below); the recording agrees
with that older one but for the cases that depend on the date format (the
older one ran in `YYYY-MM-DD`, this one in the default `MM/DD/CCYY`), the
`RAND()` one (its two columns are counted now), a backslash-`n` in a string
(`'a\qb'`), 12000 nested parentheses (50 now: tflip's parser recurses deeper
than a test thread's stack allows), a left-deep chain of 128 `+` (a balanced
tree now), and how many rows of a failing cursor are read. The commands whose
answers tflip does not give as the original does are listed in
`claims_known.txt` and `claims_huge_known.txt` (a `# <case>` line above each
group).
`fixtures/sql/claims_cases.json` is that older recording: one object per case
name, holding each statement's result as the engine returned it.

`fixtures/sql/select.json` is the same for
[`tools/ace/sql/select.txt`](../tools/ace/sql/select.txt)
(`run.sh <work> <dll> select.txt`): aggregates, `GROUP BY` and `HAVING`,
joins, `UNION` and subqueries, mostly on the tables in `fixtures/sql/tables/`
(`cust` and `sales`), which the original engine made from
[`tools/ace/sql/tables.txt`](../tools/ace/sql/tables.txt) in an empty folder
(`sql.exe <dll> <folder> tables.txt`). `run.sh` copies them into the data
folder for all the scripts.

`fixtures/sql/dml.json` is the same for
[`tools/ace/sql/dml.txt`](../tools/ace/sql/dml.txt): `INSERT`, `UPDATE` and
`DELETE`, the rows they affected and the tables after each, mostly on
`fixtures/sql/dml/items` (an AutoInc column, a unique and a plain index, a
memo field), which the original engine made from
[`tools/ace/sql/dml_tables.txt`](../tools/ace/sql/dml_tables.txt); `run.sh`
copies it into the data folder for this script only, and the replay runs on
a copy of its own.

`fixtures/sql/merge.json` is `MERGE` on a free connection
([`tools/ace/sql/merge.txt`](../tools/ace/sql/merge.txt), in an empty
folder): both clauses and each alone, a source that is the table itself
(read as the statement goes), a source row matching two target rows, no
`USING`, `WITH DELETE` and `WITH RECALL` on ADT, Clipper, FoxPro and Visual
FoxPro tables, the counts, the errors and `SHOW PLAN`. The tables and index
files it leaves are in `fixtures/sql/merge/`, which the replay compares byte
for byte. `fixtures/sql/mergedd.json`
([`mergedd.txt`](../tools/ace/sql/mergedd.txt)) is the same on a dictionary
connection, with defaults, constraints, a primary key, an AutoInc column and
triggers, and the dictionary and tables it leaves in
`fixtures/sql/mergedd/`.

`fixtures/sql/updfrom.json` is `UPDATE … FROM` with joins and `UPDATE … WITH
DELETE | WITH RECALL`
([`tools/ace/sql/updfrom.txt`](../tools/ace/sql/updfrom.txt), in an empty
folder): which row of a join that is not one to one updates a record, the self
join and the table a name in `UPDATE` means, outer joins, the order of the
errors, and the marks on ADT, Clipper, FoxPro and Visual FoxPro tables (with
memo values, whose blocks an ADT record `WITH DELETE` frees). The tables and
index files it leaves are in `fixtures/sql/updfrom/`, which the replay
compares byte for byte.

`fixtures/sql/dbf.json` is the same for
[`tools/ace/sql/dbf.txt`](../tools/ace/sql/dbf.txt): the statement
settings (`tabletype`, `chartype`, `locktype`, `readonly`, `rights`: the
values they refuse, and refusals while a cursor is open), and on a Clipper,
a FoxPro and a VFP table the script makes in an empty folder, `SELECT`s
(blank and NULL values, an ADS VarChar continued in the memo file,
aggregates, a self-join, an index's lookups and order), deleted records,
read-only and OEM cursors, tables of another type (5025, 5004), and empty and
blank values of each table type in `IS NULL`, `IFNULL`, comparisons,
aggregates, groups, `DISTINCT`, a join, a `UNION` and a static cursor. The
replay runs it in an empty folder of its own.

`fixtures/sql/plans.json` is the same for
[`tools/ace/sql/plans.txt`](../tools/ace/sql/plans.txt): the plan cursors
`SHOW PLAN FOR` returns (tflip's plans differ by design where the original
uses indexes; the replay compares only their statement row then).

`fixtures/sql/percent.json` is the same for
[`tools/ace/sql/percent.txt`](../tools/ace/sql/percent.txt): `TOP n PERCENT`
(rounding, 0 and over 100 percent, after `DISTINCT` and `GROUP BY`, in a
`UNION`, a derived table and a subquery, `START AT`, counts past 32 bits) on
ADT tables the script makes in an empty folder.

`fixtures/sql/iif.json` is the same for
[`tools/ace/sql/iif.txt`](../tools/ace/sql/iif.txt): which arguments `IIF`,
`CASE`, `COALESCE` and `IFNULL` evaluate, with a division by 0 in the one not
taken, on a table the script makes in an empty folder.

`fixtures/sql/wherefilter.json` to `wherefilter6.json` are the same for
[`tools/ace/sql/wherefilter.txt`](../tools/ace/sql/wherefilter.txt) to
`wherefilter6.txt`: a `WHERE` run as a filter on a table's records, which
evaluates both sides of `AND` and `OR` and every branch of `IIF` and
`COALESCE` (a division by 0 fails whichever operand does it, at the open of a
cursor or a statement); which conjuncts and which functions the original's
expression engine has (the others make a static cursor, evaluated as the SQL
engine does, lazily); and constant conditions. On tables the scripts make in
an empty folder.

`fixtures/sql/tsarith.json` and `tsarith2.json` are the same for
[`tools/ace/sql/tsarith.txt`](../tools/ace/sql/tsarith.txt) and
`tsarith2.txt`: arithmetic and comparisons of a `TIMESTAMP`, a `DATE` and a
`TIME` with numbers, with each other and with text (a `TIMESTAMP` takes none,
a `DATE` plus or minus a number is a `DATE`), on a table the scripts make in
an empty folder. `fixtures/sql/staticorder.json` is
[`tools/ace/sql/staticorder.txt`](../tools/ace/sql/staticorder.txt): which
static cursors have an index of their own for their `ORDER BY` and which are
read through an order of the table, on a table with three orders in an empty
folder. `fixtures/sql/params2.json` is
[`tools/ace/sql/params2.txt`](../tools/ace/sql/params2.txt): parameters in
`CASE` and `IIF` (refused), `IN`, `BETWEEN`, `LIKE`, `AND`, `CAST` to a date
and after a sign or `NOT` (also of a name typed earlier), on the default
data.

`fixtures/sql/literals.json` is the same for
[`tools/ace/sql/literals.txt`](../tools/ace/sql/literals.txt) (in an empty
folder, plus a dictionary `sy.add` for its last statements): `TIME`, `DATE`
and `TIMESTAMP` literals and escapes (a 12-hour time, a `T` between date and
time, the forms the original refuses with 2109 and 2118), text assigned to
and compared with dates, times, timestamps and GUIDs, `CURRENT_TIMESTAMP(n)`,
`AVG` of integers, a unary `+` and `-`, a subquery's column that only the outer
table has, memos and binaries in `DISTINCT`, `GROUP BY` and `ORDER BY`,
`ORDER BY` of a `UNION`, and `RIGHT JOIN` of a system table or a subquery.
(`INLIST` is not an SQL function: its 2158 is there, and its cases for the
expression engine, with more than 25 arguments and a NULL among them, are
in `fixtures/expr/expressions.json`.)

`fixtures/sql/modulo.json` is the same for
[`tools/ace/sql/modulo.txt`](../tools/ace/sql/modulo.txt): the precedence and
sign of `%` (it is on the level of `*` and `/`, left to right, not below them
as the help ranks it).

`fixtures/sql/gram_shift.json` is the same for
[`tools/ace/sql/gram_shift.txt`](../tools/ace/sql/gram_shift.txt) (the
`gram_*` scripts run in an empty folder and make their own tables): the shifts
`<<` and `>>`. They are on the level of `|`, `&` and `^`, left to right; the
result has the left operand's type (a NUMERIC of no decimals is a SMALLINT, an
INTEGER or a BIGINT by its digits); a count of the width or more leaves 0 (or
-1, shifting a negative number right) and a negative count shifts the other
way; the lowest number of a width is no value, which stored in a 16 or 32 bit
column fails with 5179 and is NULL in a BIGINT; a `WHERE` with a shift gives a
static cursor.

`fixtures/sql/gram_bin.json` is the same for
[`tools/ace/sql/gram_bin.txt`](../tools/ace/sql/gram_bin.txt): binary
literals, `x'2A 2B'`. Spaces between the digits are dropped, any other
character or an odd number of digits is 2109, and the value is a VARBINARY of
its length (one for none); one in a `WHERE` gives a static cursor, and compared
with a Raw column it is compared byte for byte, the column's zero padding
included. A Raw column is set from one with `AdsSetFieldRaw`'s padding, but
too long a value is 5071 and writes nothing.

`fixtures/sql/gram_script.json` is the same for
[`tools/ace/sql/gram_script.txt`](../tools/ace/sql/gram_script.txt) (in an
empty folder, plus a dictionary `gs/gs.add` for the bodies of procedures): the
script forms `ELSEIF`, `ENDIF`, `ENDWHILE`, `ENDTRY`, `END TRY` and a bare
`END`, which closes the innermost `IF`, `WHILE` or `TRY`. An `END` followed by
another block's word is 2117 (`found unexpected tag after END block`), and an
end nothing opened is 2115 — `END` and `ELSE` with the shorter list of what a
statement starts with, the other words with the longer one.

`fixtures/sql/gram_oj.json` is the same for
[`tools/ace/sql/gram_oj.txt`](../tools/ace/sql/gram_oj.txt): the ODBC escape
`{OJ …}` in `FROM`, a join of its own (outer, inner or bare `JOIN`; a `CROSS`
or `NATURAL JOIN` still wants its `ON`) that may stand in a comma list or on
either side of another join, takes no alias after the `}` and holds no other
`{OJ`; the errors for what is missing (the `OJ`, a join, an `ON`, the `}`),
and a `{OJ` in the select list (2118) or in place of the table of an
`INSERT`, `UPDATE` or `DELETE`.

`fixtures/sql/gram_cast.json`, `gram_cast2.json` and `gram_cast3.json` are
the same for [`tools/ace/sql/gram_cast.txt`](../tools/ace/sql/gram_cast.txt),
`gram_cast2.txt` and `gram_cast3.txt`: a constant number, text or bytes where
a logical is wanted (`1 AND 1`, `NOT 1`, in a `WHERE`, `IIF`, `CASE` and a
script's `IF`), `CAST` and `CONVERT` to the names without `SQL_` and the
sizes the types take, bytes converted to numbers and the reverse, `COALESCE`
of bytes, `ABS` and the bitwise operators, and the syntax errors of a
missing operand.

`fixtures/sql/gram_syntax.json` is
[`tools/ace/sql/gram_syntax.txt`](../tools/ace/sql/gram_syntax.txt): where
and how the original words syntax errors (a missing operand, `DATE '…'`,
`!=`, a call's missing `)`, the clauses of `INSERT`, `UPDATE` and `MERGE`,
`TOP` with a fraction), the sizes of `CAST` types, a condition that is none,
`HAVING` without `GROUP BY`, and the rows of a static cursor that hold a
value too large for their column.

`fixtures/sql/params3.json` is
[`tools/ace/sql/params3.txt`](../tools/ace/sql/params3.txt): text for a
number in `CAST(? AS …)`, `IIF`, `IN`, comparisons and sums, and numbers and
dates given to a parameter that is a date, and a parameter of a statement
that does not parse. `fixtures/sql/guid.json` is
[`tools/ace/sql/guid.txt`](../tools/ace/sql/guid.txt): text parameters
compared with a GUID column and written to one, with braces or without, and
text that is no GUID. `fixtures/sql/ddprocs2.json` is
[`tools/ace/sql/ddprocs2.txt`](../tools/ace/sql/ddprocs2.txt): the types of a
procedure's parameters beyond `ddprocs.txt`'s (`VARCHARFOX(n)`,
`VARBINARYFOX(n)`), and its dictionary in `fixtures/sql/ddprocs2/`.
`fixtures/sql/spddri2.json` is
[`tools/ace/sql/spddri2.txt`](../tools/ace/sql/spddri2.txt): the refusal of
a `RESTRICT` rule with and without a message, and a rule made over rows that
break it (5139). `fixtures/sql/planupd.json` is
[`tools/ace/sql/planupd.txt`](../tools/ace/sql/planupd.txt): `SHOW PLAN FOR`
`UPDATE` (with and without `FROM`), `DELETE` and `INSERT`.

`fixtures/sql/gram_charlen.json` is the same for
[`tools/ace/sql/gram_charlen.txt`](../tools/ace/sql/gram_charlen.txt):
`CHAR_LENGTH` and `CHARACTER_LENGTH` with `USING CHARACTERS | OCTETS |
CODE_POINTS` (only these take it; a comma or a second `USING` is a missing
`)`, and a unit that is none is 2115), and `OCTET_LENGTH` and `BIT_LENGTH`
in bytes: a CHAR, VARCHAR, CICHAR or memo is as long in octets as in
characters, a NCHAR, NVARCHAR or NMEMO twice as long (the units of
`CHARACTERS` and `CODE_POINTS` differ only for a character of two UTF-16
units, which the script cannot hold). A BINARY or IMAGE column is refused
(2159, `Argument cannot be LONGVARBINARY.`), a RAW or a number too.

`fixtures/sql/gram_sysvar.json` is the same for
[`tools/ace/sql/gram_sysvar.txt`](../tools/ace/sql/gram_sysvar.txt):
`::conn.TerminalClientAddress` (a VARCHAR(1) holding the empty text, as
`::conn.ClientHostName` and `::conn.NetworkAddress` do on a local connection;
read only) and `::stmt.unicode_sort_key_size`, which can be set (an integer
from 1 to 8, else 2227; a text is 2124, a logical value 2124 with its reason)
but not read: a `SELECT` or a script's assignment of it fails as for any name
that is no variable (2122 and 2121). It has no effect on what a sort returns.

`fixtures/sql/scripts.json` is the same for
[`tools/ace/sql/scripts.txt`](../tools/ace/sql/scripts.txt): a script sent as a
statement (several statements, or one that starts with `DECLARE`, `IF`, ...) and
`EXECUTE IMMEDIATE` — the cursor of the last statement, the rows the statements
wrote together, what a failing statement leaves written, the error positions
(whole text, with line and column after the first line), comments, `;;`,
parameters across the statements, the script language's own checks (a late
`DECLARE`, a statement after `RETURN`) and what `EXECUTE IMMEDIATE` runs and
reports — on ADT tables the script makes in an empty folder.

`fixtures/sql/cursors.json` and `fixtures/sql/trycatch.json` are the same for
[`tools/ace/sql/cursors.txt`](../tools/ace/sql/cursors.txt) (cursors in scripts:
`DECLARE c CURSOR`, `OPEN`, `FETCH`, `CLOSE`, `c.column`, the loop's end, a cursor
closed, opened twice or read past its end) and
[`tools/ace/sql/trycatch.txt`](../tools/ace/sql/trycatch.txt) (`TRY ... CATCH ...
FINALLY`, `__errcode`, `__errtext`, `__errclass`, `RAISE` with no arguments),
on ADT tables the scripts make in an empty folder.

`fixtures/sql/cursorlive.json` is the same for
[`tools/ace/sql/cursorlive.txt`](../tools/ace/sql/cursorlive.txt): what a cursor
in a script sees of the changes the script makes to its tables after `OPEN`
(updates, inserts and deletes, with and without `ORDER BY`, with a `WHERE` and
subqueries, over a table with an index) on tables it makes in an empty folder.
`fixtures/sql/cursormore2.json` and `fixtures/sql/cursormore3.json` are the same
for [`cursormore2.txt`](../tools/ace/sql/cursormore2.txt) (a cursor sorted by a
column the table has an index for, which reads through the index a leaf of keys
at a time) and [`cursormore3.txt`](../tools/ace/sql/cursormore3.txt) (a cursor over
an inner join of two tables on one equality, with and without an index on the
second table's column).
`fixtures/sql/cursormore4.json` is the same for
[`cursormore4.txt`](../tools/ace/sql/cursormore4.txt): a cursor over a `LEFT JOIN`,
a join with an `ORDER BY` or a condition beyond the equality, a join of three
tables, `UNION ALL` and a `SELECT` in `FROM`, each case on tables made new.
`fixtures/sql/cursormore5.json` is the same for
[`cursormore5.txt`](../tools/ace/sql/cursormore5.txt): a join with no equality (a
cross join, `ON t.n < u.g`), a `RIGHT JOIN`, a `LEFT JOIN` with a condition beyond
its equality, a `SELECT` in `FROM` that is joined or sorted, and a join of three
tables with an index or a condition on one of them.
[`cursormore.txt`](../tools/ace/sql/cursormore.txt), the first probe of these
cursors, is not replayed.
`fixtures/sql/derived.json` is the same for
[`derived.txt`](../tools/ace/sql/derived.txt): a `SELECT` of one table in
`FROM` in its forms, live or static, with an edit through the cursor after
each, and the plans of `COUNT(*)` and of derived tables.
`fixtures/sql/cursorprocs.json` and `fixtures/sql/cursortrig.json` are the same
for [`tools/ace/sql/cursorprocs.txt`](../tools/ace/sql/cursorprocs.txt) and
[`tools/ace/sql/cursortrig.txt`](../tools/ace/sql/cursortrig.txt): cursors in the
bodies of procedures and triggers, over a `SELECT`, over `EXECUTE PROCEDURE` of
a script procedure and of a system procedure, in a dictionary each script makes.
`fixtures/sql/loops.json` is the same for
[`tools/ace/sql/loops.txt`](../tools/ace/sql/loops.txt): `LEAVE` and `CONTINUE`
in `WHILE` loops of scripts, procedures and triggers, nested and inside `IF`,
`TRY`, `CATCH` and `FINALLY`, and what the language refuses of them.
`fixtures/sql/gram_assign.json` is the same for
[`tools/ace/sql/gram_assign.txt`](../tools/ace/sql/gram_assign.txt): numbers with
more decimals than a NUMERIC column or variable holds, as literals, expressions
and parameters, and prepared parameters read as other types.

`fixtures/sql/props39.json` is the same for
[`tools/ace/sql/props39.txt`](../tools/ace/sql/props39.txt): the unpublished
`AdsSetProperty` id 39 that ARC sets before `AdsVerifySQL` (the next verify, and
the same text again straight after, checks the syntax alone; any value arms it;
only a statement takes it), in an empty folder.
[`tools/ace/sql/props.txt`](../tools/ace/sql/props.txt) probes ids 27 and 87 and
makes no fixture: id 87 writes 0 with the stock DLL and 1 with the AES-capable
patched ones, on handle 0 and on connections, statements and cursors alike (an
invalid non-zero handle faults the original), so its value depends on the DLL.

`fixtures/sql/temptables.json` is the same for
[`tools/ace/sql/temptables.txt`](../tools/ace/sql/temptables.txt): temporary
tables (`#name`) made and read by SQL — names without case, `CREATE INDEX`,
`ALTER TABLE`, `UPDATE`, `DELETE`, a join, a script, `DROP TABLE`, what a name
that is not there answers (7112), and the type of the statement that made one —
in an empty folder.

`fixtures/sql/temptables_api.json` is the same for
[`tools/ace/sql/temptables_api.txt`](../tools/ace/sql/temptables_api.txt):
temporary tables made by `AdsCreateTable` and opened by `AdsOpenTable` (`ctable`
and `otype` commands of `sql.c`) as each table type, in an empty folder; the
table made by SQL as one type and opened as another (7016, 7028).
`fixtures/sql/temptables_types.json` is the same for
[`tools/ace/sql/temptables_types.txt`](../tools/ace/sql/temptables_types.txt):
what statements do with such tables (`SELECT`, DML, `ALTER`, `DROP`, `CREATE
INDEX`, joins), which depends on whether the statement's last `SELECT` read the
table; in an empty folder.
`fixtures/sql/temptables_api2.json` and `temptables_api3.json`
([`temptables_api2.txt`](../tools/ace/sql/temptables_api2.txt),
[`temptables_api3.txt`](../tools/ace/sql/temptables_api3.txt)): a Clipper or
FoxPro temporary table opens as either until a statement has opened it, and as
that statement's type only after; `SELECT … INTO #name` is such a statement.
`fixtures/sql/temptables_into.json`, `temptables_into2.json`,
`temptables_into3.json` and `temptables_into4.json`
([`temptables_into.txt`](../tools/ace/sql/temptables_into.txt) and the
three that follow it): `SELECT … INTO #name` in each table type: the table's
type and name, what it holds, the fields of each result type in the DBF types,
the types they refuse (5012) and the names of more than ten characters; in an
empty folder.

`fixtures/sql/ddproclong.json` and the dictionary in `fixtures/sql/ddproclong/`
are the same for [`tools/ace/sql/ddproclong.txt`](../tools/ace/sql/ddproclong.txt):
`CREATE`, `ALTER` and `DROP PROCEDURE` with parameter lists longer than the
dictionary record's 275-byte property field, which the original continues in the
memo file (`More Property`). `fixtures/dictionary/proclong/` is the same for
`AdsDDAddProcedure` and `AdsDDSetProcedureProperty`, made by
[`tools/ace/dd/proclong.sh`](../tools/ace/dd/proclong.sh) (`proclongapi`, and
`proclongext` for lists that fill more than 32 memo blocks); the replays compare
the `.add`, `.am` and `.ai` byte for byte: the original sets a procedure's
properties one at a time, so a continuation moves in the memo file as the
properties grow, leaving stale bytes and counting each allocation.
`fixtures/dictionary/removelong/` is the same for objects with a `More Property`
continuation that are removed (the original frees it, then the comment's memo
block; `removed`) and for extents taken from the free runs views leave
(`extents`), made by [`tools/ace/dd/removelong.sh`](../tools/ace/dd/removelong.sh).
`fixtures/dictionary/proplong/` is the same for property setters that make an object's
properties longer than the 275-byte field (`viewlater`, `proplater`) or short again, made
by [`tools/ace/dd/proplong.sh`](../tools/ace/dd/proplong.sh).
`fixtures/dictionary/deploy/` is the same for `AdsDDDeployDatabase`, made by
[`tools/ace/dd/deploy.sh`](../tools/ace/dd/deploy.sh): per scenario the source
dictionary and its tables (`before/S`), the free tables it is deployed to
(`before/D`) and what the original left there (`after/D`; the source's own files
are the ones of `before/S` renamed `adsbackup`), and in `results.txt` the
arguments and the result of each call. The scenarios are tables that match,
that are missing, that gain, lose and change fields, that gain index files and
orders and lose orders, memo fields, DBF tables of the three types, a backup
with the folder's files and one over a backup that is there, a view, and the
refusals of a destination that holds a dictionary's tables (5159) or the
source's own name (5128); tables in folders of the destination, there or made
with their folders (`sub_a`, `subm_a`, `subm_b`); dictionaries with users and
groups (`obj_user`, `obj_group`, `x_users`, `x_sqlusers`), rights (`x_grant`,
`x_dropped`), procedures, functions, packages, views and triggers (`x_big`,
`x_trigger`, `x_long`), descriptions (`x_desc`), a referential integrity rule
(`x_ri`), a link (`x_link`), a publication (`x_pub`) and settings of the
database (`x_dbset`), which the original adds to a new dictionary, each object
with a new id; and `validate_option` 2, which takes the records that break the
dictionary's constraints out of the tables into fail tables (`constraint*`,
`fail_exists`, `v_*`, and `o*` for the order of the constraints). The probe
adds scenarios that show what tflip leaves undone (`x_pub*`: rights granted to
`DB:Public`) without fixtures. The deployment's log has its times and the
folder it names replaced by `00:00:00` and `{D}`; the dictionary's database
login secrets, which the original salts anew, are not compared.
`fixtures/sql/spddlong.json` and the dictionary in `fixtures/sql/spddlong/` are
the same for [`tools/ace/sql/spddlong.txt`](../tools/ace/sql/spddlong.txt):
functions, procedures, views, triggers and tables with columns whose scripts,
parameter lists and messages run past the property field, in the `de/` of the
`spdd*` scripts.

`fixtures/sql/declare.json` is the same for
[`tools/ace/sql/declare.txt`](../tools/ace/sql/declare.txt): the variables of
`DECLARE` — each type, the names that are none (`No such type`), the sizes each
takes and the largest, what a value does assigned to each type (the kinds it
takes, the numbers that do not fit, text cut to length, the date, time and GUID
read from text) and what a variable of another variable's value is refused —
in an empty folder.

`fixtures/sql/sysvars.json` is the same for
[`tools/ace/sql/sysvars.txt`](../tools/ace/sql/sysvars.txt): the system
variables `::conn.*`, `::stmt.*` and `::server.OldestActiveTxn` in a statement
and in a script (their types and columns, `UpdateCount` through a script's
statements, what is none), asked about where they differ from run to run (the
names, the user, the operation count), in an empty folder.
`fixtures/sql/trigvars.json` is what a trigger reads of the change that fires
it (`tools/ace/sql/trigvars.txt`: `::stmt.Trig*`, the `ROWID`, the record number
of an `INSTEAD OF` insert), on a copy of the psm dictionary, recorded on the
remote server like `triggers.json`. `fixtures/sql/trigtyped.json` is a
trigger body's script variables in its statements
(`tools/ace/sql/trigtyped.txt`: a NULL `CHAR` variable, numbers and dates
read by an `INSERT`), on a copy of the psm dictionary, recorded on the local
engine. `fixtures/sql/trigerr.json` is a trigger's `__error` table
(`tools/ace/sql/trigerr.txt`: the error number and text it writes there, the
rows it changes, the timings and events, nested triggers), on a copy of the
psm dictionary, recorded on the local engine (replayed with counted
transactions). `fixtures/sql/trigproc.json` is a trigger body calling stored
procedures (`tools/ace/sql/trigproc.txt`: arguments from `__new`, variables and
subqueries, output parameters and cursors that go nowhere, how a procedure's
failure reads in the trigger's error, the pseudo-tables a procedure sees, a
system procedure), on a copy of the psm dictionary, recorded on the local engine
like `trigerr.json`; `fixtures/sql/trigproc2.json` is how deep a trigger that
writes its own table goes, with none to three procedures between
(`trigproc2.txt`: 64 levels, a trigger taking 2 of them and the first 4).
`fixtures/sql/trigorder.json` is the order the triggers of one timing and
priority fire in (`trigorder.txt`, made by `trigorder.py`: creation orders,
names, drops and re-creations, `ALTER`, priorities, timings), and
`trigorder2.json` the triggers made right after the dictionary's own were
dropped (`trigorder2.txt`); both on a copy of the psm dictionary, recorded on the
local engine, with `system.triggers` as the original lists them (by record).
`fixtures/sql/trigbig.json` is a trigger whose script is exactly 65,535 bytes
(`tools/ace/sql/trigbig.txt`, made by `trigbig.py`: every statement of its
event fails with 5005, the other events do not, an `ALTER` to such a script),
in a dictionary of its own, whose files are compared with
`fixtures/sql/trigbig/` (the script's slot is the length 0 and no bytes), recorded
on the local engine. `fixtures/sql/trigdb.json` is `CREATE`, `ALTER` and `DROP TRIGGER … ON
DATABASE` (`tools/ace/sql/trigdb.txt`: the database events and the timings they
take, the refusals, names, and the dictionary `fixtures/sql/trigdb/` the script
leaves, compared byte for byte), in an empty folder, recorded on the local
engine. `fixtures/sql/trigdbfire.json` is database triggers firing on `CONNECT`
and `DISCONNECT` (`tools/ace/sql/trigdbfire.txt`: their `__info`, a failing
`CONNECT` trigger refused with 7109 but for `ADSSYS`, `RAISE` and `__error` in
them), and `fixtures/sql/trigdbopen.json` is `OPEN_TABLE` ones that fail, cancelling
the open (`tools/ace/sql/trigdbopen.txt`), and `fixtures/sql/trigdblog.json`,
`trigdblog2.json` and `trigdblog3.json` are what `OPEN_TABLE` and `CLOSE_TABLE`
triggers that log see (`tools/ace/sql/trigdblog.txt`, `trigdblog2.txt`,
`trigdblog3.txt`: which tables each statement opens and when it closes them, the
open count, a client's open of a cursor, triggers that fail, procedures and table
triggers, a second connection, `UserID` and `StmtCount`), and
`fixtures/sql/trigself.json` is triggers writing their own table (a `BEFORE` trigger's writes fire it again, an
`AFTER` or `INSTEAD OF` one's fire none of that event) and transaction
statements in a trigger (`tools/ace/sql/trigself.txt`), all in an empty folder,
recorded on the local engine. `fixtures/sql/castchar.json` is how wide `CAST(x AS SQL_CHAR)` makes
a value of each type and how it writes it (`tools/ace/sql/castchar.txt`), in
an empty folder; `fixtures/sql/castchar2.json` the same for the types of table columns,
TINYINT, REAL, BIGINT literals, memos and GUIDs (`tools/ace/sql/castchar2.txt`).
`fixtures/sql/params.json` is the types the original gives
the parameters nothing else types (`tools/ace/sql/params.txt`: `- ?`,
`COALESCE`, `TIMESTAMPADD`, `? IS NULL`, text longer than 1024) and the
values a re-prepared statement keeps. `fixtures/sql/params4.json` is which
values a re-prepared statement keeps, by position and by name, and what a
failed prepare or run does to them (`tools/ace/sql/params4.txt`);
`fixtures/sql/params5.json` that a name that comes twice in the SQL is two
parameters (a name sets both, a number one) and what `AdsGetNumParams`
counts: every marker of any text prepared, the most of them, which
`AdsClearSQLParams` does not lower (`tools/ace/sql/params5.txt`).
`fixtures/sql/procvars.json` is what a
stored procedure reads and how it fails (`tools/ace/sql/procvars.txt`: the
table `__input`, `UpdateCount` through a procedure and the ones it calls, the
ACE error 5154 around what failed, the arguments it refuses, the types of its
output columns), in a dictionary it makes. `fixtures/sql/subq.json` is the
subqueries in a script's own expressions (`tools/ace/sql/subq.txt`), in an
empty folder. `fixtures/sql/udf.json` is user-defined functions that run SQL
(`tools/ace/sql/udf.txt`), on a copy of the dictionary the original made of
them, `fixtures/sql/udf/` (`tools/ace/sql/udfdict.txt`; tflip makes no
functions with SQL yet).

`fixtures/sql/groupconcat.json` is the same for
[`tools/ace/sql/groupconcat.txt`](../tools/ace/sql/groupconcat.txt):
`GROUP_CONCAT` (`DISTINCT`, an inner `ORDER BY`, `SEPARATOR`, what it refuses,
the 1024-byte limit), over a `CICHAR` column too, with the case-insensitive
type the text functions of one give, on ADT tables the script makes in an
empty folder.

`fixtures/sql/funcs.json` is the same for
[`tools/ace/sql/funcs.txt`](../tools/ace/sql/funcs.txt): `DATABASE()`,
`NEWID()`, `NEWIDSTRING()` and the connection's `RAND()` sequence — seeds, row
by row, in `WHERE`, and with `TOP` (which works out the select list only for
the rows it keeps) — on a table the script makes in an empty folder.

`fixtures/sql/rowfuncs.json` is the same for
[`tools/ace/sql/rowfuncs.txt`](../tools/ace/sql/rowfuncs.txt): `RECNO`,
`DELETED`, `EMPTY` (on ADT, CDX, NTX and VFP tables), `ROWNUM` (in `WHERE`, with
`TOP`, `GROUP BY`, joins and `UNION`), the `ROWID` column, `LASTAUTOINC`,
`LASTROWID`, `LASTROWVERSION`, `APPLICATIONID`, `TTOC` and `COLLATE`, also in
the `WHERE` and `SET` of `UPDATE` and `DELETE`, on tables the script makes in an
empty folder (the replay compares only the record number of a `ROWID`).
`fixtures/sql/rowid_paths.json` has the `ROWID`s that
[`rowid.txt`](../tools/ace/sql/rowid.txt), [`rowid2.txt`](../tools/ace/sql/rowid2.txt)
and [`rowid3.txt`](../tools/ace/sql/rowid3.txt) gave in work folders of known
paths (the connection's path, the table's file or object id, the `ROWID`).

`fixtures/sql/spview.json` is the same for
[`tools/ace/sql/spview.txt`](../tools/ace/sql/spview.txt): `sp_GetColumns` on
a dictionary's views and `sp_GetTables`/`sp_GetColumns` following its links,
connected to a copy of the `dictionary_props` dictionary in `views/` that the
script adds views and a link (to the `dict/` dictionary) to.

`fixtures/sql/spdict.json` is the same for
[`tools/ace/sql/spdict.txt`](../tools/ace/sql/spdict.txt): `sp_GetTables` and
`sp_GetColumns` on a connection to the `dict/` dictionary (no links), which list the
dictionary's own tables and views and the columns with their dictionary defaults,
comments and nullability.

`fixtures/sql/views.json` is the same for
[`tools/ace/sql/views.txt`](../tools/ace/sql/views.txt): which views of a
dictionary the script makes in the SQL folder (`vw.add`, over `indexed` and
`cust`) open a live cursor on their table and which a static one (a column
subset, a `WHERE`, renamed and repeated columns, a view of a view, a view of
a `SELECT` in `FROM`; expressions, `TOP`, `DISTINCT`, a join, `LIKE`,
`{static}`, `UNION` and `GROUP BY` are static), what a `SELECT` on a view
does in turn, `AdsSqlPeekStatement` and the plans, and the cursors' indexes
(all the table's, as a live cursor on the table itself has them). The edits
and appends the script makes through the cursors, which the original takes
into the table (a static cursor's it refuses with 5072), come last: tflip
refuses them all, so the replay compares nothing after the first.

`fixtures/sql/orderby.json` is the same for
[`tools/ace/sql/orderby.txt`](../tools/ace/sql/orderby.txt): the temporary
index of an `ORDER BY` — on which cursors it is added (live and static, and
the shapes that are not), its name, key expression (a descending key among
others, expressions, aliases, constants left out, which expressions an index
takes), flags, key length, type and count, what a client can do with its
handle (moves, seeks, `AdsGetIndexHandleByExpr`, `AdsOpenIndex`), and that
the cursor's order stays as it is after moves through the table's other
orders. The edits come last, as in `views.json`.

`fixtures/sql/oemrows.json` is
[`tools/ace/sql/oemrows.txt`](../tools/ace/sql/oemrows.txt): text a script,
a subquery, a derived table or a variable reads out of an OEM table (the `co`
table of `charset_default` alone, opened with `ADS_OEM`), which is ANSI.
`fixtures/sql/likeorder.json` is
[`tools/ace/sql/likeorder.txt`](../tools/ace/sql/likeorder.txt): the order of
the rows of a `LIKE`, alone and beside other conditions, with an index on
its column, on another or none (it makes no difference);
`fixtures/sql/likedict.json` is
[`tools/ace/sql/likedict.txt`](../tools/ace/sql/likedict.txt): the same on a
dictionary connection, in a table of the dictionary (ordered) and in its
`system.` tables (not); and
`fixtures/sql/statics.json` is
[`tools/ace/sql/statics.txt`](../tools/ace/sql/statics.txt): each scalar
function of the help's lists ("Indexes with Expressions") in a `WHERE` and in
an `ORDER BY`, whether the cursor is the table's (live, its file the
table's, and it takes `AdsSetString` and `AdsWriteRecord`) or a static one (a
temporary table, the write refused with 5072). All three run with
[`run.sh`](../tools/ace/sql/run.sh) in a data folder of their own.

`fixtures/sql/indexsel.json` is
[`tools/ace/sql/indexsel.txt`](../tools/ace/sql/indexsel.txt): the rows and
plans of ranges, `IN` lists and joins on tables with orders on one field, on
several, on an expression and on CICHARACTER text, in an empty folder;
`fixtures/sql/indexsel2.json` is
[`indexsel2.txt`](../tools/ace/sql/indexsel2.txt): a lone `>=` or `<=` with
and without an order, on text of characters that sort differently in lower
and in upper case; `fixtures/sql/indexsel3.json` is
[`indexsel3.txt`](../tools/ace/sql/indexsel3.txt), the same on the `ger` and
`gen` tables of `collation_dynamic` under their collations, run with
[`run_collation_dynamic.sh`](../tools/ace/sql/run_collation_dynamic.sh); and
`fixtures/sql/joinorder.json` and `joinsize.json` are
[`joinorder.txt`](../tools/ace/sql/joinorder.txt) and
[`joinsize.txt`](../tools/ace/sql/joinsize.txt): which table of an inner join
the original reads first, by the tables' record counts, restrictions and
orders.

`fixtures/sql/links.json` is the same for
[`tools/ace/sql/links.txt`](../tools/ace/sql/links.txt): tables reached through
a dictionary's links (`link.table`) and dictionary paths in `SELECT`, `INSERT`,
`UPDATE` and `DELETE`, with the linked dictionary's logins, rights and rules,
and what fails; run in a folder holding copies of the two dictionaries alone
(`props/` and `dict/`), which the script changes.

`fixtures/sql/linklist.json` is the same for
[`tools/ace/sql/linklist.txt`](../tools/ace/sql/linklist.txt), in the same
folder: a dictionary's links outside SQL — the tables of linked dictionaries in
`AdsFindFirstTable62` and `AdsDDFindFirstObject` listings (of
`ADS_DD_TABLE_VIEW_OR_LINK_OBJECT` and the other table, view and link types),
for the administrator and for users with few rights; tables named through a
link given to `AdsOpenTable`; and `AdsGetNumActiveLinks`/`AdsGetActiveLinkInfo`
as links are made, used, listed and dropped (the length the latter writes back
counts the data folder as the 6 bytes of `<data>`).

`fixtures/sql/txscripts.json`, `txprocs.json` and `txtrig.json` are
transactions in SQL, recorded on the remote server like `triggers.json`
([`txscripts.txt`](../tools/ace/sql/txscripts.txt): `BEGIN TRANSACTION`,
`COMMIT`, `ROLLBACK` and savepoints in scripts and around `TRY ... CATCH`, a
`COMMIT` with none begun, a failing statement, what a script leaves open;
[`txprocs.txt`](../tools/ace/sql/txprocs.txt): the same in stored
procedures, in a dictionary it makes (the count a procedure restores, what
it may end, its savepoints, failing and nested procedures);
[`txtrig.txt`](../tools/ace/sql/txtrig.txt): a trigger's savepoints and
levels, `NO TRANSACTION` on one of an event's triggers, and a referential
integrity rule inside the caller's transaction after it changed the child).
Where the remote server answers a `COMMIT` or `ROLLBACK` with 5047 though it
carries it out (`txscripts.json`'s `/* sequence */` statements) is not
replayed.
`fixtures/sql/txtrig2.json`
([`txtrig2.txt`](../tools/ace/sql/txtrig2.txt), the same way) is a trigger's
savepoints: an `AFTER INSERT` trigger that rolls back to a savepoint it set
takes back the insert it fired on (the other events keep their change), and
a `NO TRANSACTION` trigger in the caller's transaction is a call as a
procedure is (7129 for the caller's savepoints, 5047 for a `COMMIT`). Each
`CREATE TRIGGER` follows a `closestmt` and a new statement: the original goes
on with the triggers a table had when a statement opened it, until the
statement closes. A cursor's record count after a taken back insert is the
original's own (0, -1 or the slot's), so it is not compared.
`fixtures/sql/txtrig3.json` ([`txtrig3.txt`](../tools/ace/sql/txtrig3.txt)) is a trigger
created or dropped on a second connection while a statement of the first holds the
table open, in a dictionary the script makes (replayed). `fixtures/sql/trignn.json`
([`trignn.txt`](../tools/ace/sql/trignn.txt)) is the order of a field's own `NOT NULL`
and the `BEFORE INSERT` triggers that fill the field, in a dictionary the script makes (replayed).
`fixtures/sql/txddl.json` is DDL in a transaction
([`txddl.txt`](../tools/ace/sql/txddl.txt), replayed);
`fixtures/sql/txlimit.json` the statement limit (7216,
[`txlimit.txt`](../tools/ace/sql/txlimit.txt), a unit test, not replayed) and
`fixtures/sql/txremote.json` what only a remote server shows
([`txremote.txt`](../tools/ace/sql/txremote.txt): `::conn.ClientHostName`,
`NetworkAddress`, `sp_GetSecurityInfo`'s communication type, `CREATE ONLINE
INDEX` and `ALTER ONLINE TABLE`; not replayed, `tflipd` having no client host
name to give).

`fixtures/sql/triggers.json` is the same for
[`tools/ace/sql/triggers.txt`](../tools/ace/sql/triggers.txt), recorded on
the remote server (the 12.0 evaluation server under Wine, `SQL_SERVER_TYPE=2`)
on a copy of the `psm` dictionary alone, whose triggers the script drops:
`CREATE TRIGGER`/`DROP TRIGGER`, CONFLICT triggers (made, refused on `INSERT`
and twice, never fired by the table's own changes), which timings of an
event go together, `NO MEMOS` (memo values NULL in `__new`/`__old`), and
what a failing trigger takes back with and without `NO TRANSACTION`, in and
out of the client's transaction, with the transaction count a trigger
reads.

`fixtures/events/*.json` are event notifications across several connections
and threads, recorded by [`tools/ace/events/ev.c`](../tools/ace/events/ev.c)
(`run.sh`) on the remote server like the transaction scripts, each script
making its own dictionary in an emptied folder of the server:
`events_basic` (`sp_CreateEvent`, `sp_SignalEvent` from a procedure,
`sp_WaitForEvent` and `sp_WaitForAnyEvent` with and without a timeout, the
counts, the order, a signal before the wait), `events_data` (events with
data, `sp_DropEvent`, `sp_DropAllEvents` and every procedure's errors),
`events_trigger` (a trigger signalling with `::stmt.TrigRowID`, the help's
example), `events_scope` (whose events are whose: one dictionary however its
path is written, free connections by their folder, another dictionary),
`events_tx` (a signal that waits for the commit, a connection that registers
again after closing, waits with a timeout in scripts and procedures) and
`events_wait` (a wait signalled while it waits, and cancelled by
`sp_CancelQuery`; the timings in it are the original's, and not compared).
`events_tx_local.json` and `events_wait_local.json` are the same two scripts
on the local server, whose transactions are nothing and which cancels
nothing. Four more are about what only a server shows of several
connections, recorded with `ev.c`'s `hold` (a statement left open on the
table): `events_statements` (`sp_GetSQLStatements` lists every client's
statements on the same dictionary or folder, a wait not active and a long
query active, 5054 for a user without rights), `events_trigger_shared` (a
trigger one connection creates or drops applies to another's next
statement), `events_trigger_open` (one created while another's statement
holds the table open fires for none until no connection has the table open;
one dropped stops at once; not replayed), `events_online` (CREATE INDEX and
ALTER TABLE leave the table unopenable to a second connection, 7008, the
ONLINE forms leave it open, and `ONLINE_WITH_TRANSITION` waits for the
connections that opened the table meanwhile; on 131072 records, replayed
through `tflipd` in about a minute), `events_online_final` (the same for a
table of a dictionary, on 65536 records, where the refusal names no file,
and the events `__OCIFinalStage_<table>` and `__OAFinalStage_<table>` that
CREATE ONLINE INDEX and ALTER ONLINE TABLE signal at their final stage and
the offline forms do not), `events_online_args` (the arguments, options and
errors of `sp_PackTableOnline`, `sp_ReindexOnline` and
`sp_PackAllTablesOnline`: `Timeout`, `Indexes` and `MemoBlockSize` entries
separated by `;`, 5019, 5065, 5058, 7041) and `events_online_procs` (what they
leave open to other connections, `__OPFinalStage_<table>` for a pack, for each
table of a pack of all, and none for a reindex, and a transaction on the table
holding the final stage back until it ends or the `Timeout` does, 7176 and
7189) and `events_tx_sub` (7130 and 7187, tried again
and not seen; not replayed). They are replayed by
`crates/tflip-ace/src/events.rs`, the others locally and through `tflipd`.
[`events_online_sort.txt`](../tools/ace/events/events_online_sort.txt) is the
sequence of ADS_BUGS's unconfirmed stop of the server (a sorted `SELECT` after
`ALTER ONLINE TABLE`), run eighteen times over without it stopping (no fixture).

Two more groups of `fixtures/events/*.json` are about what the remote server
does with logins and with free tables, replayed through `tflipd` only.
`lockout_*` (`tools/ace/events/lockout_*.txt`) are the bad-login lockout
(`ENFORCE_MAX_FAILED_LOGINS`, `MAX_FAILED_ATTEMPTS`): one dictionary and one
or two users in each, and a few failed connects, spaced by `pace` steps and a
good login before each, because the server resets the circuit (6624) after
failures that follow one another, and at times after others (a run that has
one is made again, none is in a fixture). `freeconf_*`
(`tools/ace/events/freeconf_run.txt`, once with no setting, with
`Check_Free_Table_Access` and with `Disable_Free_Connections`, set in the
service's registry key and the service restarted) run in
`fixtures/events/freeconf/`, the folder `freeconf_setup.txt` made on the server
(free ADT and DBF tables, and a dictionary with a table, a view and a
procedure over a free one, and a user). `freeconf_check_proc.json` is not
replayed.

`fixtures/sql/fts_plans.json` is the same for
[`tools/ace/sql/fts.txt`](../tools/ace/sql/fts.txt) on a copy of
`fixtures/sql/fts/` alone: full-text plans (live and static, `CONTAINS` among
other predicates, words as long as an index's longest), `SCOREDISTINCT`, and
queries over a FIXED full-text index. That folder's `docs`, `opts`, `fixed`
and `nofts` tables back `fixtures/sql/fts.json`; its `fx` (a FIXED index the
table changed after), `px` and `qx` (a full-text and an ordinary index, made
in either order) were made by the original engine from
[`tools/ace/sql/fts_tables.txt`](../tools/ace/sql/fts_tables.txt) in an empty
folder (`run.sh <work> <dll> fts_tables.txt`).

`fixtures/sql/collation_dynamic.json` is the same for
[`tools/ace/sql/collation_dynamic.txt`](../tools/ace/sql/collation_dynamic.txt),
made by
[`run_collation_dynamic.sh`](../tools/ace/sql/run_collation_dynamic.sh) under
the default static collation on the `charset_german` `ca` table (without its
index) and the `collation_dynamic` `ger` and `gen` tables: text compared,
sorted, grouped and joined under the dynamic collations a statement
(`AdsStmtSetTableCollation`) or the connection (`AdsSetCollation`) sets —
GERMAN, UNIQWT, MACHINE and CZECH — literals' too, and the 5209 of a table
whose orders have another; needs the collation files (skipped without them).

`fixtures/sql/collation_case.json` is `UPPER` and `LOWER` of each byte 32 to 255 of
the `charset_german` `ca` table under `ANSI_CHAR_SET=German`, distilled from the run of
[`tools/ace/sql/collation.txt`](../tools/ace/sql/collation.txt) by
[`run_collation.sh`](../tools/ace/sql/run_collation.sh). `collation_compare.json` and
`collation_compare_default.json` are
[`collation_compare.txt`](../tools/ace/sql/collation_compare.txt) under
`ANSI_CHAR_SET=German` and under the default static collation (made by
[`run_collation_compare.sh`](../tools/ace/sql/run_collation_compare.sh)): `WHERE`,
`ORDER BY`, `DISTINCT`, `GROUP BY` and `UNION` on the same table's text, and on a
CICHARACTER copy of it.

## Field getters

`fixtures/fields/getters.json` is what the original engine's typed getters
(`AdsGetLong`, `AdsGetLongLong`, `AdsGetShort`, `AdsGetDouble`, `AdsGetMoney`,
`AdsGetLogical`, `AdsGetJulian`, `AdsGetMilliseconds`, `AdsGetMemoLength`,
`AdsGetFieldNum`, `AdsIsFieldBinary`, `AdsGetString(W)`, `AdsGetDate`,
`AdsGetTime`) return, recorded by
[`tools/ace/fields/fields.c`](../tools/ace/fields/fields.c) under Wine on
copies of the `all_types` and `adi_keys` fixtures: every getter on every
field of every record and at BOF and EOF, then null pointers, short
buffers, options (also for `AdsGetField(W)`), date formats and other
handles. Outputs start as `0x5A` bytes and are recorded whatever the return
code; text is hex.

`fixtures/fields/dates.tsv` is which date the original engine reads from
text in date formats with and without separators, recorded by
[`tools/ace/fields/dates.c`](../tools/ace/fields/dates.c) from the texts in
[`dates.txt`](../tools/ace/fields/dates.txt): per line the format, the
text, AdsSetDate's and AdsSetField's return code on a Date field and the
Julian day after each (1000 when the set failed), then AdsSetField's return
code on a TimeStamp field and its text after — texts with a time among
them, on where the date ends and the time starts.

`fixtures/fields/tstexts.tsv` is which timestamp the original engine reads
from the texts of `dates.txt` and
[`tstexts.txt`](../tools/ace/fields/tstexts.txt) (no date, no time,
two-digit years under other epochs, times written in other ways), recorded
by [`tools/ace/fields/tstexts.c`](../tools/ace/fields/tstexts.c): per line
the format, the epoch and the text, then `CTOTS()` of it (`DTOS()` of the
result, in brackets, and its time in milliseconds), AdsSetField's return
code on a TimeStamp field and the Julian day and milliseconds after it,
and AdsSeek's return code for it as an `ADS_STRINGKEY` in an order on that
field and the key it sought: the Julian day and milliseconds, found by
moving the table's one record about (`~` when no value a record can hold is
the key: it lies just before that one).

## Writes

`fixtures/write/<scenario>/` records what the original engine writes when a
client changes tables through the ACE API. Each scenario has two scripts for
[`tools/ace/write/write.c`](../tools/ace/write/write.c): `setup.txt` makes
the tables in an empty folder (SQL `CREATE TABLE`, `CREATE INDEX`, ...), and
`ops.txt` writes to them (`append`, `set`, `write`, `delete`, `lock`, ...),
and reads what filters make of them (`aof`, `rows`, ...).
[`tools/ace/write/run.sh`](../tools/ace/write/run.sh) runs both under Wine
and keeps the folder after the setup as `before/` and after the writes as
`after/`, with each command's return code and outputs in `result.json`.
tflip's tests replay `ops.txt` on a copy of `before/` and expect `after/`
byte for byte, and the same results. Not compared are the bytes that hold
the time of a write (ADT ModTime fields, the date of the last update in a
DBF header) and those listed in a scenario's `volatile.json`: `run.sh` runs
the ops twice, with the probe's stack filled with different bytes, and
lists the bytes that differ, which the engine left uninitialised (so far
two bytes of a table in `ddl`). Some bytes the engine fills from its own memory
whatever the stack holds, so they are the same in every run: the byte after
a binary value in a `.dbt`. tflip lists the bytes it writes there
(`tflip_storage::undefined`, with the feature `undefined` the tests turn
on), and those are not compared either; nor are the bytes of an `.ntx`
page that hold nothing (past its last item, and the record and key of the
item that holds its rightmost child), where the engine leaves what its page
buffers held. The `tx_*` scenarios ran on
the remote server, the ADS 12 evaluation server under Wine (`run.sh` with
`WRITE_SERVER_DIR` and `WRITE_SERVER_PATH`), and keep snapshots of the files
in the middle of the script (`snapshot <name>` in `ops.txt`) in
`snaps/<name>/`, with the bytes that differed between the two runs in
`snaps-volatile.json`. The replay compares the tables in the snapshots,
and every file in `after/` (the original's index and memo files in the
middle of a transaction are not what its server holds). A scenario's
`known.json` lists, by command number, the answers not compared and why
(differences KNOWN_ISSUES.md lists; a known code takes its error text
with it). Each failing command's `AdsGetLastError` text is compared, the
data folder in it written as `<data>` with backslashes, as `write.c`
writes it. Two scenarios leave out what
depends on the original's per-handle caches: `locks` and `dbf_records` do
not ask a second handle for its record count while it has not refreshed.

| Scenario | What it covers |
| --- | --- |
| `records` | every field type outside the memo file through the typed setters and `AdsSetField`; appends, an update written by moving off the record, a delete and an append into its slot, a cancelled append, explicit and implicit record locks, empty values, AutoInc, RowVersion and ModTime |
| `setters` | every setter (`AdsSetString`, `AdsSetField`, `AdsSetLong`, `AdsSetDate`, ...) on every type outside the memo file, with typical, malformed, too long and out-of-range values: the return code, `AdsGetField`'s text and the field's bytes after each |
| `locks` | two handles on a table: record and table locks between them, appends from both at once, a delete and an append that reuses its slot; read-only and exclusive opens |
| `memos` | memo, binary and NMemo values through the ACE API with block sizes 8, 64, 256 and 1024 and in COMPRESSED fields: sizes around the `.adm` allocator's boundaries, then a seeded random run of updates, NULLs, deletes and appends (made by [`tools/ace/write/memos.py`](../tools/ace/write/memos.py)); a change cancelled, a delete with a change pending |
| `memo_setters` | every setter on Memo, NMemo and binary fields |
| `binary_pieces` | `AdsSetBinary` in pieces (`ulTotalLength`, `ulOffset`) on Binary and VarBinary fields: a first piece padding with zeros, a piece inside the value, a cut, NULL (total and length 0), a length of 0, the offsets and lengths refused (5082, before the record is looked at), the types refused (5066, 5070), a field that is no binary one, a compressed field refusing chunks (5227); only growth after a cut is left out, where the original exposes the bytes it cut (`DIFFERENCES.md`) |
| `binary_pieces_image` | the same on an Image field (type 7, and 6 refused with 5066) |
| `binary_pieces_records` | pieces on several records, a value set by `AdsSetString` before a piece, appended, cancelled, deleted and recalled records |
| `guid_formats` | `AdsGetField` of a GUID field with `ADS_GET_GUID_MIME`, `_FILE`, `_NUMBERS` and `_REGISTRY` (and other bits, and two of them: 5019 "Unknown GUID encoding option"), buffers of every length around the text, a NULL GUID, other field types |
| `guid_formats_more` | the same with the trim, UTF-8 and ANSI bits set too, other fields, and the record checked before the option |
| `handle_kinds` | `AdsGetHandleType` of a free-table connection (1), a dictionary connection (6), a table (2), an index order (3), a full-text index order (8, by `AdsGetIndexHandle`) and a cursor (5) |
| `copy_made_by_client` | `AdsGetLastError` after `AdsCopyTable`, `AdsCopyTableContents` and `AdsConvertTable` of tables opened exclusively: `AE_INFO_COPY_MADE_BY_CLIENT` (5169) with a return code of 0, "The source table was opened exclusively" or "The index order used for copying was opened exclusively"; none for a structure, a shared or default open, or a conversion to another type |
| `indexes` | records written to a table whose structural index has orders of many kinds (a field, descending, two fields, `UPPER()`, a `FOR` condition, custom, binary `DELETED()`), 512-byte pages: 500 appends, then a seeded random run of changes, deletes and appends (made by [`tools/ace/write/indexes.py`](../tools/ace/write/indexes.py)); duplicate keys in a unique order |
| `dbf_records` | Clipper (NTX), FoxPro (CDX) and Visual FoxPro tables with every field type outside the memo file, through the typed setters and `AdsSetField`: appends, a change written by moving off the record, NULL and empty values, a cancelled change and append, deletes and recalls with and without `AdsShowDeleted`, VFP AutoInc, record and table locks between two handles, read-only and exclusive opens, and OEM tables (made by [`tools/ace/write/dbf.py`](../tools/ace/write/dbf.py)) |
| `dbf_setters` | every setter on every field type of each DBF kind, with the values of `setters` |
| `dbf_oem` | every ANSI byte through `AdsSetString` and `AdsSetStringW` on a Character and a Memo field of OEM Clipper and FoxPro tables: the client's code page 437 as the engine stores it (best fit, or 5211) |
| `dbf_memos` | memo, binary and NMemo values of each kind: DBT sizes around its 512-byte blocks, FPT block sizes 64, 512, 33 and 1000, a seeded random run of updates, NULLs, deletes and appends, values grown past the FPT free list's 82 entries, ADS VarChar overflows, VFP VarChar and VarBinary, and every setter on memo fields |
| `ntx_indexes` | records written to Clipper tables with twelve `.ntx` indexes each (fields, `UPPER(c) + DTOS(d)`, `FOR` conditions with `DELETED()`, `UNIQUE`, descending, `STR(n) + c`): a seeded random run of appends, changes, deletes and recalls; a Double too wide for its key (4004); a table grown to 3,000 records (made by [`tools/ace/write/xindex.py`](../tools/ace/write/xindex.py)) |
| `ddl` | `CREATE TABLE` of every field type, memo files of four block sizes, compressed memo fields, a duplicate column name, and refused definitions; orders created on empty tables of many kinds (`CREATE INDEX`, `CREATE UNIQUE INDEX`, `DESC`, two columns, `sp_CreateIndex90` with expressions, conditions, custom and binary orders, other index files and page sizes, `sp_CreateIndex`); records then added; `DROP INDEX` and `DROP TABLE` |
| `build` | orders of every kind of `ddl` built on tables of 1, 10, 120, 500 and 3,000 records (made by [`tools/ace/write/build.py`](../tools/ace/write/build.py)), some deleted, some NULL: multi-level trees of 512, 1,024 and 8,192-byte pages; a unique order the keys refuse (7059); records added to built orders |
| `fts_build` | full-text orders (`CREATE INDEX … CONTENT`) built on tables of tricky text (case, accents, quotes, numbers, a word longer than the key, tabs and line ends, noise words, NULL, a deleted record) in a character and a memo field, with each option (`MIN WORD`, `MAX WORD`, `CASESENSITIVE`, `KEEPSCORE`, `NOTMAINTAINED`, `DELIMITERS`, `PROTECTNUMBERS`, `NOISE`, `DROPCHARS`, `CONDITIONALS`, with and without `NEW`); trees of several levels with and without scores; one added to a file of 512-byte pages; how given characters and noise words merge with the defaults, and the options refused (5019) (made by [`tools/ace/write/fts.py`](../tools/ace/write/fts.py), like the one below) |
| `fts_rebuild` | full-text orders made with `AdsCreateFTSIndex` (each "use default" flag with a list, NULL and empty lists, word lengths and options at their edges, the refusals 5019 and 5066, an order replaced, VarChar and CIChar fields), in the table's index file and in files of their own (pages of 1,024 by default, 512 and 8,192 given), then rebuilt by `AdsReindexFTS` (its own pages kept for 0; a file with an ordinary order left), `AdsReindex`, `AdsReindex61` to 2,048-byte pages, a pack and a zap, records written between (`tools/ace/write/fts.py`); the scored order is in a file of its own, see `docs/ADS_BUGS.md` |
| `fts_writes` | records appended, changed (to other words, to NULL, the count of a word) and deleted in tables with full-text orders (kept current, keeping scores, case-sensitive with short keys, `NOTMAINTAINED`) beside an ordinary order: a memo table, an empty character table filled, and a seeded random run of 400 writes on a table of 300 records, so that leaves split and empty |
| `fts_binary` | full-text orders on Binary, Image and VarBinary fields of an ADT table, made by SQL (default, `KEEPSCORE`, `CASESENSITIVE` with short words, `NOTMAINTAINED`) and by `AdsCreateFTSIndex`, beside an ordinary order; records appended, changed (to other words, to NULL, from empty), deleted and recalled, so that a value set at once in the memo file keeps its old words for the order; words with a zero byte, a byte above 0x7F and a control character (a zero byte splits words whatever the delimiters); a seeded random run of 300 writes on 300 records, then `AdsReindex`, `AdsReindexFTS` and a pack (`tools/ace/write/fts.py`) |
| `fts_recall` | records of ADT tables deleted and all recalled at once (`AdsRecallAllRecords`) with full-text orders on a character field (keeping scores), a memo field, and a memo field beside an ordinary order |
| `fts_recall_binary` | the same on Binary and Image fields, with records recalled one by one and all at once, and one deleted and recalled all again |
| `fts_cdx_create` | full-text orders on FoxPro tables made by SQL (a tag of the table's own `.cdx`, which is made for it where there is none, beside an ordinary tag, and with `KEEPSCORE`), and by `AdsCreateFTSIndex` (a single-order file of its own: the table's `.idx` for none, else the file named), over a table with two records, and over none; the tag header's full-text mark and the offset of the options block, which comes before the root in a compound file and after it in a single one |
| `fts_cdx` | the same orders on a FoxPro and a VFP table kept current: records appended, changed, deleted and recalled; the orders made by SQL (default, `KEEPSCORE`, `CASESENSITIVE` with short words, `NOTMAINTAINED`, on a Blob field) and by the API (the refusal of an Integer field, 5066, leaves the other order of the file); a table of 300 records written at random, `AdsReindex`, `AdsReindexFTS`, a pack and a zap |
| `fts_cdx_null` | a VFP Character field set to NULL, to empty text and to other text, a record deleted and one with a field changed, beside two full-text orders: a NULL keeps the field's bytes, so the order keeps the record's words |
| `fts_tx_rollback`, `fts_tx_commit` | full-text orders (default, `KEEPSCORE`, on a character field) beside an ordinary order, on the remote server, in a table a transaction appends to, changes (memo and character fields), deletes from, writes to by SQL, goes back to a savepoint in, and leaves a change pending in, rolled back and committed, a table for each (made by [`tools/ace/write/fts_tx.py`](../tools/ace/write/fts_tx.py)): what `CONTAINS` finds on the connection that writes and on another meanwhile (the orders hold the words of before until the commit, which takes the writes in one by one; a record the connection deleted is gone from its own answer), the files in between and after. Replayed |
| `fts_tx_big_rollback`, `fts_tx_big_commit` | the same on a table of 160 records with two character fields: 60 appends, 70 changes and 20 deletes at random, so that the orders' leaves split (the commit takes the orders in the order of their tags' names) |
| `fts_tx_memo_rollback`, `fts_tx_memo_commit` | the same with a memo field and fewer writes (the orders on the memo field before those on the character field; the commit frees the memo values of the images a record had between; a deleted slot keeps the record's last image) |
| `fts_tx_cdx_rollback`, `fts_tx_cdx_commit`, `fts_tx_vfp_rollback`, `fts_tx_vfp_commit` | The first scenario of the ADT ones on FoxPro and Visual FoxPro tables (made by [`tools/ace/write/fts_tx.py`](../tools/ace/write/fts_tx.py)), on one connection and with the files only, as tflip's SQL finds nothing by `CONTAINS` on a DBF table yet: appends, changes and deletes by the API and SQL, a savepoint and changes pending, rolled back and committed; the commit takes a record's words once, from its original to its final image. Replayed |
| `fts_tx_exclusive` | `AdsReindexFTS`, `AdsReindex`, `AdsPackTable` and `AdsZapTable` in a transaction, on a table opened exclusively: 5063. Replayed |
| `maintenance` | `AdsPackTable`, `AdsZapTable` and `AdsReindex` on a table with deleted records, memo values and orders in two index files (refused on a shared open); `AdsCreateTable71` with type names cut short, an AutoInc's first value and refused definitions; `AdsCreateIndex61` on a table with a record (replacing an order of the same name, in another file, with a condition) and `AdsDeleteIndex` |
| `table_ops` | The smaller table calls: `AdsRecallAllRecords` (with memo values, then an AOF set aside and put back with `AdsDeactivateAOF` and `AdsActivateAOF`), `AdsPackTable120` with a new memo block size and refused sizes and options, `AdsDeleteTable` on a shared open, `AdsSetTableTransactionFree`, `AdsFlushFileBuffers`; keys: `AdsLookupKey`, `AdsBuildKeyFromRecord`, raw keys (`AdsInitRawKey`, setters on the order, `AdsBuildRawKey100`'s three lengths), `AdsStepIndexKey`, `AdsGetKeyColumn`; `AdsSetRecord`, `AdsSetFieldRaw`, `AdsAccessVfpSystemField` on a VFP table; the connection's switches without a dictionary |
| `locate_custom` | `AdsContinue` (before any `AdsLocate`, forwards to EOF, backwards, from a record moved to); `AdsAddCustomKey` and `AdsDeleteCustomKey` on a custom order (a second key for a record, a key added while the record is changed); `AdsFileToBinary` into Binary and Image fields and refused for others; `AdsSetStringFromCodePage` from code pages 1252 and 437 |
| `restructure` | `AdsRestructureTable120` and `ALTER TABLE` on free ADT tables: fields added (at a position, an AutoInc), deleted, renamed, resized and moved, an indexed table's orders following them (the old files left as `.BAK`); the refusals (no such field, a field that exists, an unknown type, a field an order uses, a table open elsewhere, a conversion that fails part way); memo block sizes; a value of each common type converted to others |
| `bitmaps` | A binary order whose bitmap takes three pages, built on a table of 9,000 records with deleted ones, then rebuilt by `AdsReindex` after another order |
| `cdx_bitmaps` | Binary CDX orders (`DELETED()` and `x > 6000`) whose bitmaps take three nodes, built on a FoxPro table of 11,900 records, walked across their leaves, kept current as appends start a fourth leaf and records are deleted, recalled and changed, then rebuilt by `AdsReindex`; and a bitmap that outgrows its root node. Its snapshots are taken with the tables closed, so their index files are compared too |
| `bitmap_split` | (made by [`tools/ace/write/bitmap_levels.py`](../tools/ace/write/bitmap_levels.py), like the three below) A binary order whose bitmap fills the 62 leaves of its root node on a table of 246,015 records (nodes of 512 bytes), with the first record past them: the leaves move under an interior node and a second one takes the new leaf; records deleted in both, then `AdsReindex`. The table has no deleted records, as an append reuses one |
| `bitmap_levels` | Binary orders on a table of 492,031 records, whose bitmaps take 124 leaves under two full interior nodes, with a second order built after the first; a 125th leaf starts a third interior node; records deleted in each, then `AdsReindex` |
| `cdx_bitmap_split` | `bitmap_split` for binary CDX orders (`DELETED()` and `x > 'a'`) on a FoxPro table, whose changed records flip the bit of the second |
| `cdx_bitmap_levels` | Binary CDX orders of 76 leaves (a full interior node and one of 14 leaves) and of 124, kept current past the 124th leaf (a third interior node), then rebuilt. The tables are of records of 2 bytes, to keep the fixture small |
| `dbf_create` | `AdsCreateTable71` for Clipper, FoxPro and Visual FoxPro tables: every type each takes, memo files and their block sizes, VFP's `NULL`, `NOCPTRANS`, AutoInc and `_NullFlags`; the types each refuses; a record written to some |
| `ntx_create` | `AdsCreateIndex61` on Clipper tables: NTX orders on no records (a field of each type; a compound one refused), on three (unique with a condition, an expression, descending; a record written after) and on 3,000 (trees of two and three levels) |
| `cdx_create` | `AdsCreateIndex61` on FoxPro and VFP tables: tags of the structural index (the table's header then naming it) on no records, on four (unique, descending, a condition, an expression, Date and Logical keys; a record written after), on 3,000, and in another compound file; VFP Integer keys |
| `limits` | The limits `AdsCreateTable71` and `AdsCreateIndex61` enforce on ADT, Clipper, FoxPro and VFP tables, made by [`limits.py`](../tools/ace/write/limits.py): field lengths and decimals of each type, memo block sizes, field, tag and expression names and lengths, key lengths (a key too long for a new ADI file's pages, an ADI file that grows its pages for one, and the page a new one takes by default), 15 NTX files open, 256 tags (up to 60 are made), and the table a failed create leaves current |
| `tagdir` | A tag directory of more than one page, made by [`tagdir.py`](../tools/ace/write/tagdir.py): the 256 tags an ADI file and a CDX file take (the directory splits its leaves under interior nodes at the end of the file, before the tag's root leaf), and the 257th, refused with 5046 |
| `tagdir_drop` | The same with 100 tags a file: opening them, appending a record, dropping them in two sweeps (leaves of the directory emptied and unlinked), making new ones and searching what is left |
| `dbf_maintenance` | `AdsPackTable`, `AdsZapTable` and `AdsReindex` on Clipper and FoxPro tables with memo values, deleted records and two orders each (refused on a shared open) |
| `dbf_rebuild` | `AdsReindex` and `AdsPackTable` on a FoxPro table of 3,000 records with two orders and deleted records: the `.cdx` file written anew with trees of many leaves |
| `dbf_recall` | `AdsRecallAllRecords` on FoxPro and Clipper tables whose orders leave deleted records out |
| `restructure_rename` | `AdsRestructureTable120` on ADT tables with fields that are only named again (an indexed field of a table with a memo file, a name that differs in case, two at once): the table's header alone is written and no `.BAK` made; the same names with a type, a position, or a name kept, restructure |
| `dbf_restructure` | `AdsRestructureTable120` on FoxPro, Clipper and VFP tables: fields added, deleted, renamed, resized and changed to text, a deleted record kept, memo values written anew, the structural `.cdx` rebuilt with a renamed field in its expressions, NTX files left alone; a field an order needs (refused); VFP fields that take NULL |
| `cdx_kinds` | `AdsCreateIndex61` on a FoxPro table: single-order `.idx` files (plain, unique, descending with a condition), candidate orders (refused), a custom order, binary orders (a Logical field, `DELETED()`) |
| `cdx_custom_reindex` | `AdsReindex` on FoxPro and ADT tables with custom orders (emptied, the keys clients added gone, ADT tag headers counting the writes), `AdsAddCustomKey` on a FoxPro order, a restructure refused for a custom order (5152), candidate orders on a VFP table, one over a NULL (7153) |
| `binary_changes` | ADT tables with an order on `id` and a binary `DELETED()` order, four records each: deletes and recalls (written, moved off, re-gone-to after a failed `AdsGotoRecord` 5022, `AdsRecallAllRecords`), changes that leave the bits alone, appends and a cancelled append, then the binary order walked before and after `AdsReindex`, whose tag headers count the writes (made by [`tools/ace/write/binary_changes.py`](../tools/ace/write/binary_changes.py)) |
| `reindex_pages` | `AdsReindex61` on ADT tables with four orders of 200 records (a binary one, a custom one) with page sizes 0, 1, 255 to 8,193, 16,384, 65,536 and 4,294,967,295 (5019 outside 512 to 8,192, checked before the open mode, 5067), the file kept at 512 or 2,048 bytes (made by [`tools/ace/write/reindex_pages.py`](../tools/ace/write/reindex_pages.py)) |
| `append_reuse` | Which record an ADT append takes after deletes in various orders (the free list is a LIFO stack), a recall of the head, the tail or the middle (the stale link ends the list: the append goes past the last slot and the header's list and count are dropped), `AdsRecallAllRecords`, a pack, a zap, a reindex, a cancelled append, in a transaction committed, and with another handle's lock on a deleted record (5035); `AdsRecallAllRecords` also writes the orders' change counts to the tag headers (made by [`tools/ace/write/append_reuse.py`](../tools/ace/write/append_reuse.py)) |
| `autoinc_reset` | The next value of an ADT AutoInc field after `sp_ZapTable`, `AdsZapTable`, a `DELETE` of every record or some, a pack, and an Integer field made AutoInc by `ALTER TABLE` and `AdsRestructureTable` (with values, none, zeros, duplicates); the values appended and the header's counter (made by [`tools/ace/write/autoinc_reset.py`](../tools/ace/write/autoinc_reset.py)) |
| `cursor_writes` | `AdsAppendRecord`, `AdsWriteRecord`, `AdsDeleteRecord`, `AdsRecallRecord`, `AdsCancelUpdate` and `AdsWriteAllRecords` on a live SQL cursor of an ADT table (`cursor <select>` of [`write.c`](../tools/ace/write/write.c)): the AutoInc value, a column subset, the deleted slot an append takes, a recall that cannot lock a deleted record (5035), the table's file written as its own handle writes it |
| `cursor_refused` | The same calls on cursors that are not written: a static one, a join's, and a live one of a read-only statement (5072; `AdsWriteRecord` and `AdsWriteAllRecords` succeed, `AdsCancelUpdate` is 5085, the record locks 5064 where the table is not shared), and the calls no cursor takes: `AdsLockTable`, `AdsUnlockTable`, `AdsPackTable`, `AdsZapTable`, `AdsReindex` (5115) |
| `cursor_where` | Live cursors with a `WHERE` or an `ORDER BY` (a temporary order, an order of the table, a `WHERE` an index answers): an appended record that does not meet the `WHERE` stays, can be gone to and is not counted; a changed record that leaves it; a delete in it; the order keeping the records appended and moved; the table's index file written |
| `cursor_dbf` | The same on a VFP table (deleted records stay shown, a recall of a written delete works) |
| `cursor_constrain` | `AdsStmtConstrainUpdates(ADS_CONSTRAIN)`: a write that leaves the cursor's `WHERE` fails with 7065 and the record stays changed (a move, a count and the next append fail with it as well), an appended one taken back when the cursor closes; with `ADS_NO_CONSTRAIN`, with an `ORDER BY`, with an index serving the `WHERE`, and on a column subset |
| `cursor_settings` | `AdsStmtConstrainUpdates` and `AdsStmtReadAllColumns`: 5109 with a cursor open, 5118 and 5019 for a value that is none of theirs, no change in the columns a cursor has on a local server; a cursor's invalid column is 5113 and 5112 for a getter as for a setter |
| `cursor_locks` | A live cursor's `AdsLockRecord`, `AdsUnlockRecord`, `AdsIsRecordLocked` and `AdsGetAllLocks` against a plain handle and another connection's (5035, `AdsRefreshRecord`, locks kept by an append) |
| `cursor_views` | Cursors on a dictionary's tables and views (a view of all columns, a `WHERE`, a column subset, aliases, a view of a view): a table's field defaults on an append, a view's columns and aliases to write by, `ADS_CONSTRAIN` through a view |
| `vfp_orders` | `AdsCreateIndex61` on a VFP table: orders on fields that take NULL (keys a byte longer), candidate orders, one on such a field leaving NULL out |
| `cdx_drop` | `AdsDeleteIndex` on the orders of a structural `.cdx` (FoxPro and VFP): an order's name leaving the tag directory, an unknown order (5016), a record written in between, and the last order, which takes the file and the table's flag with it |
| `sql_dbf` | SQL on Clipper, FoxPro and VFP tables, by the statement's table type (`tabletype`, `AdsStmtSetTableType`): `CREATE TABLE` of the types each takes, `INSERT` (a NULL a VFP field refuses), `UPDATE`, `DELETE`, `CREATE INDEX` (unique, descending, two columns as `NAME+DtoS(D)`), `sp_CreateIndex90` without a file and with an `.idx`, `DROP INDEX`, `ALTER TABLE ADD COLUMN`, `DROP TABLE`; tables of another type (5004, 5025) |
| `tx_basic` | Transactions on the remote server (the evaluation server under Wine): a change, an append and a delete rolled back, a change with a memo value committed, a savepoint; another connection reading and locking meanwhile; `snaps/` holds the files after each step (the `.adi` files there are stale: the server caches index pages). Replayed |
| `tx_dbf` | Transactions on FoxPro, VFP and Clipper tables on the remote server: a change, an append (with a memo value) and a delete rolled back, then committed, with snapshots during, after the rollback and after the commit. Replayed |
| `tx_handles` | Two handles of one ADT table on one connection and a third on another, on the remote server: a record locked through one handle cannot be locked or written through the other (5035), nor unlocked (5062), outside a transaction and in one; a record written in a transaction is locked by the handle that wrote it. Replayed (two reads of a record the handle was on already differ, `known.json`) |
| `tx_multi` | Two ADT tables in one transaction on the remote server: unique and plain keys changed, another connection appending a key the transaction took (7057), what it sees meanwhile (counts, keys), a rollback, then a commit. Replayed |
| `goto_deleted` | `AdsGotoRecord` to a deleted ADT record: refused (5022), but the table is on it, reads its values, and moves on from it in record order and in an order; going to the slot after the last. Only reads. Replayed |
| `set_errors` | the setters that fail (the ones of `setters`, `dbf_setters` and `memo_setters` that do, one of each field type, setter kind and error) and what `AdsCancelUpdate` answers after each: a value too long (5071) or out of range (5179), a NULL a field refuses and what a Numeric or binary field cannot read leave an update in progress; a field of the wrong type (5066), a bad date, time, GUID, logical or Money value and a LongInt text out of range leave none (5085); on an appended record the update is the append's. Also `AdsCancelUpdate` on no record (5068 "Cancelling update.") against on a record (5085), and NVarChar fields (indexed, shared, NULL, appended) behaving as the other text types. Replayed |
| `deleted_cursor` | a deleted ADT record the cursor was left on (`AdsGotoRecord` to it fails 5022): a field set on it is taken in a table open exclusively (5035 in a shared one, as a delete is) and the record is written deleted again with the new value, the table then on no record; `AdsDeleteRecord` on it likewise; recalled, it is active at once but stays on the free list, and a delete of it then, or a record that heads the free list, goes on it no more (the free list and its count stay); `AdsGetRecordCount` then counts one fewer than the file, until an append. Binary and ordinary orders' change counts, the free list's links, and counts through a reopen. Replayed |
| `error_details` | The details `AdsGetLastError`'s text adds after a failure that the other scenarios do not show: the AOF calls without an AOF (`AdsClearFilter` adds none), `AdsGotoRecord` on an ADT table, scopes not set and an option that is none (`Parameter 2`), files not found with the extension added and in a subfolder, a unique key an ADI order in a file of its own refuses (`other:UK`), a candidate order in a single-order file. Replayed |
| `skip_unkeyed` | `AdsSkip` of 1 to 6 records either way in conditional (`FOR`) orders, ascending and descending, of ADT, NTX and CDX tables, from records the condition leaves out before, between and after the keys and among keys equal to theirs, and from a deleted ADT record. Only reads. Replayed |
| `skip_deleted` | `AdsSkip` from a record the cursor was left on after a written `AdsDeleteRecord` (or a cancelled append) in an ADT table, in record order and in the order on id, deleted records shown or not (and DBF tables of NTX and CDX orders, which skip as from any record): in record order it counts from the record's slot; in an ADI order the first step goes to the key beside the record's key, each further step is two keys, so a skip of n lands on the (2n - 1)th key; a delete not yet written acts the same in the order. Recorded with a generated script (one fresh 12-record table per case). Replayed |
| `vfp_timestamps` | 446 Visual FoxPro TimeStamp values written raw (random days and milliseconds, whole seconds, times that round up to midnight) into a table with an order on the field, a second order built after them, each record's `AdsBuildKeyFromRecord` key, and hard seeks of timestamp text (keys a bit below the nearest double, milliseconds in the text). Replayed |
| `cdx_timestamps` | CDX orders on expressions whose value is a timestamp (`CREATETIMESTAMP` of the fields, `STOTS(DTOS(d))`, `CTOTS(c)`; ascending, descending, `FOR`, xBase `UNIQUE`; some on VFP fields that take NULL, and `UPPER()` of one) on a Visual FoxPro and a FoxPro table: a seeded random run of appends, changes (NULL and empty values, times with milliseconds, around midnight), deletes and recalls (made by [`tools/ace/write/cdx_ts.py`](../tools/ace/write/cdx_ts.py)), each order walked, `AdsBuildKeyFromRecord` keys, hard and soft seeks of timestamp and date text, `AdsLookupKey` by text and double; orders built on the records after; a FoxPro order built on records whose zero-ending keys the original cannot find (see `docs/ADS_BUGS.md`). Replayed |
| `open_opened` | `AdsOpenTable` with the option 0x10000 (which `AdsGetTableOpenOptions` always reports): it succeeds and hands out the handle 0, whose calls then fail, and leaves the file closed. Only reads. Replayed |
| `database_type` | `ADS_DATABASE_TABLE` and a type that is none (99) given to `AdsStmtSetTableType` and `AdsOpenTable` on a connection without a dictionary: 5025 before the file is looked for, each with its detail. Every `AdsSet*` call on a connection's handle: 5018 or 5023, with a detail for some; the original's text after `AdsSetTimeStampRaw` and `AdsSetFieldRaw` is garbled (`known.json`, see `docs/ADS_BUGS.md`). Only reads. Replayed |
| `relations` | Relations between ADT tables: refused arguments, how the child follows the parent (no key, EOF and BOF, the child moving between, the parent moving away and back), a parent in an order, partial and numeric keys, a chain, two children, cycles (5077), clearing, scoped relations and their scopes, a child closed. Only reads; the original's remote server answers the same. Replayed |
| `relations_dbf` | Relations from a FoxPro table to FoxPro and Clipper children by date, number and text keys, and a scoped one; a negative number is not found in the NTX order. Only reads. Replayed |
| `tx_pending` | Changes left pending in a handle (a change, a delete, an append, not written) when a transaction on the remote server is rolled back or committed, read by another connection after, and as the first handle moves on; a DBF delete too. Replayed |
| `tx_abandon` | A client that ends in the middle of a transaction (a change, an append, a delete): the snapshot during it, and in `after/` what the server leaves, as after a rollback. Replayed |
| `tx_append` | Appends in a transaction on the remote server, ADT and VFP: the files between `AdsAppendRecord` and the write (the marked blank record), after a cancelled append (it stays, and the commit deletes it), after a written one and the commit; another connection's counts meanwhile. Replayed |
| `tx_vfp` | VFP fields that take NULL, changed in a transaction on the remote server (`_NullFlags`, VarChar lengths, memo values), rolled back and committed. Replayed |
| `tx_dbf_memo` | Memo values set in a transaction on FoxPro, Clipper and VFP tables on the remote server: on a record with a memo block (written at the commit) and on ones without (written at once), rolled back and committed. Replayed |
| `tx_memo_exclusive` | Memo values written in transactions on an ADT table open exclusively, committed and rolled back, and on a shared one: a value that fits the block of the one it replaces, or clears it, waits for the commit; one that needs a block (a record without a value, an append, a longer one) is written at once, its block number in the marked record, and a rollback frees the blocks and keeps their data. Recorded on the remote server (`tools/ace/write/run.sh`). Replayed |
| `tx_memo_sizes` | The same with values of one block and of several (shrunk, grown, set on a record without one, deleted, appended), rolled back and committed, and appends and a change around a savepoint on an exclusive table (no 7138 there). Replayed |
| `tx_savepoint_lock` | The commit after a rollback to a savepoint that took back an appended record is 7138 though the transaction commits (not after an append into the slot again, a change, an SQL `INSERT`, a whole rollback; not on an exclusive table); the slots taken back go onto the free list newest first; the state and rows are read on a fresh handle, as the one that committed keeps its record. Replayed |
| `tx_savepoint_memo` | The same with memo values and in full-text orders: the memo blocks of the slots taken back, the words, the free list. Replayed |
| `tx_redelete` | A record deleted in a transaction and deleted or changed again (a failed `AdsGotoRecord` leaves the handle on it) on ADT, VFP and CDX tables: the transaction holds its lock, so it succeeds (shared too), and the commit keeps the values set. Replayed |
| `tx_sql` | SQL statements on the remote server (and the local one), a table per case: an `UPDATE`, an `INSERT ... SELECT` and a `DELETE` that fail part way (a key taken, a record locked), outside a transaction, in one, and in each mode of `SET TRANSACTION` (`AUTOCOMMIT_ON`, `AUTOCOMMIT_OFF`, `EXPLICIT`), read back from both connections; the mode after `BEGIN` |
| `tx_free` | The ADT free list in transactions on the remote server: a slot deleted in a transaction is not taken by an append in it (the commit frees it), a slot freed before is, a rollback gives the appended slots back in the order they were written; files byte for byte. Replayed |
| `tx_recall` | A record deleted and recalled in the same transaction (ADT, remote server): the commit still puts its slot on the free list though the record is active (`docs/ADS_BUGS.md`). Not replayed |
| `copies` | `AdsCopyTableStructure`, `AdsCopyTable` (every record, through a filter, in an order's sequence) and `AdsCopyTableContents` (by field name) on an ADT table with an AutoInc, memo values and a deleted record; a FoxPro table's deleted records copied while shown |
| `converts` | `AdsConvertTable` of an ADT table of the common field types to FoxPro, Clipper and VFP tables, and of a FoxPro table to ADT and VFP |
| `converts_ext` | `AdsConvertTable` of an ADT table of the extended field types (LongInt, GUID, Raw, VarBinary, VarChar, NChar, NVarChar, NMemo, RowVersion, ModTime) to FoxPro, Clipper and VFP, and of a FoxPro table with a short date to ADT and VFP |
| `converts_dbf` | `AdsConvertTable` of a Clipper, a FoxPro and a VFP table of every field type each takes (a record of values, an empty one, a deleted one, one of boundary values; a VFP table of NULLs) to every table type, its own included; General and Picture fields (their own kind only, 5012; a Clipper value 7019); a Double too wide for its Numeric (5179, the conversion stopped part way); a file that is there (overwritten); deleted records hidden (`AdsShowDeleted`) |
| `converts_values` | `AdsConvertTable` of ADT tables of every field type (the legacy VarChar too) to every table type, ADT included: NULL, empty and boundary values, a deleted record, Double and CurDouble fields of 0 to 20 decimals, names longer than ten characters (`FIELD<n>`), a Double too wide for its Numeric (5179), a 65,000-character field, AutoInc and RowVersion values that are not the records' numbers |
| `aofs` | AOFs (`AdsSetAOF`, also through `aof`, `aoflevel`, `evalaof`, `inaof`, `custom`, `rows` and `count`, which read rather than write) on a table with orders of many kinds and on FoxPro, Visual FoxPro and Clipper tables: the optimization level and the part not optimized of about 70 expressions, with the records each shows; then what AOFs resolved at once or as moves reach records, fixed, keyset and customized ones show, count and hold as both of two handles change, append and delete records, refreshed or not |
| `aofs_lookup` | reads only: the level, part not optimized and records of about 120 AOFs whose parts the orders serve beyond plain comparisons: `<>` on every kind of key (numbers the field rounds, doubles, NUMERIC, text with blanks, too long or empty, CICHARACTER, `UPPER()`), `YEAR()` by every comparison through ADI and CDX orders on the date and on `DTOS()` of it (fractions, years of no date, partly optimized), the first field of ADI keys of several fields, with the exact setting off and on (`exact`) |
| `aofs_collation` | reads only: AOFs on an ADT table opened under the dynamic collation `GERMAN_VFP_CI_AS_1252` (`collation`), with an order on the text and without: `=`, `==`, `<>`, `<`, `>` with case, accents and expansions (`ß` as `ss`), the exact setting off and on; needs the collation files (skipped without them) |
| `aofs_more` | reads only: the level, part not optimized and records of about 210 AOFs: `EMPTY()` and `ISNULL()` of every kind of key (also `= .T.`, `= .F.`, `<> .T.`, negated alone and with other parts), `$` on CHAR, CICHARACTER, VarChar and `UPPER()` keys (padded text, text with the key inside, empty), in ADT and FoxPro tables; the leading parts of the FoxPro keys of several fields SQL makes (`A+C`, `DtoS(D)+A`, `C+DtoS(D)`) by every comparison, with text shorter and longer than the part, `YEAR()`, `EMPTY()`, `$`; the same terms `.OR. RECNO() = 99`, which are evaluated on each record, to tell the orders' records from evaluated ones; the exact setting off and on |
| `aofs_static` | reads only: AOFs under the static collation `ANSI_CHAR_SET=German` (made with [`run_static.sh`](../tools/ace/write/run_static.sh), which runs a copy of the engine's folder with it in its `adslocal.cfg`; the `# ANSI_CHAR_SET=` line of `ops.txt` tells the replay): `=`, `==`, `<>`, `<`, `>`, `$`, `EMPTY()` on CHAR, CICHARACTER and `UPPER()` keys of an ADT table and on the text, numbers and dates of a FoxPro table, and on tables without orders; needs the collation files (skipped without them) |
| `aofs_rest` | reads only: AOFs of the terms the orders served last: `EMPTY()` and `ISNULL()` of every kind of Visual FoxPro key with NULL and blank values (and of fields that take no NULL), `EMPTY()`/`ISNULL()` `= .F.` (`<> .T.`, `== .F.`, negated, with other parts) in Visual FoxPro, FoxPro and ADT tables, and `$` through ADI and Visual FoxPro orders of the dynamic collation `GERMAN_VFP_CI_AS_1252`; each term also `.OR. RECNO() = 99` to tell the orders' records from evaluated ones; needs the collation files (skipped without them) |
| `aofs_static_keys` | reads only: AOFs under `ANSI_CHAR_SET=German` (made as `aofs_static`): CICHARACTER text of an ADI order with case, accents and expansions (`=`, `==`, `<`, `>`, `<>`, `$`, `EMPTY()`), and numbers, dates and expressions of them (`id + 1`, `DTOS(born)`) of CDX orders by every comparison, the orders walked first; each term also `.OR. RECNO() = 99`; needs the collation files (skipped without them) |
| `enforce` | `AdsDisableAutoIncEnforcement`/`AdsEnableAutoIncEnforcement` and the unique-enforcement toggles on an ADT table with an AutoInc field and one with a CANDIDATE order: with enforcement off a client's own AutoInc value is stored and the next one is kept past it, a duplicate key is accepted (7057 while it is on) (explored with [`enforce.c`](../tools/ace/write/enforce.c)) |
| `cdx_indexes` | records written to FoxPro tables with a structural `.cdx` of many orders (expressions, `FOR`, `DELETED()`, xBase `UNIQUE`, descending, binary), a second `.cdx`, eight `.idx` files and 4,000 random operations on one table, and to Visual FoxPro tables with a candidate order (7057), NULL keys, multi-segment keys and `.idx` files (TimeStamp keys are in `vfp_timestamps`, timestamp expressions in `cdx_timestamps`) |
| `nchar_indexes` | records written to ADT tables with ADI orders on NChar and NVarChar fields (ascending, descending, with a `FOR` condition, unique, on several fields): a seeded random run of appends, changes and deletes of text from many scripts (NULL and empty values too), so that leaves split, empty and go on the free list, then a pack and a reindex (made by [`tools/ace/write/nchar_indexes.py`](../tools/ace/write/nchar_indexes.py)) |
| `nchar_create` | an ADT table with NChar and NVarChar fields and orders made by `CREATE INDEX` (descending, two fields, unique, after the records) and `sp_CreateIndex90` with a condition; a record appended with UTF-16 text (`set ... stringu` of [`write.c`](../tools/ace/write/write.c)), one changed, one deleted, then `AdsReindex` |
| `uchar_keys` | ADI orders on NChar and NVarChar fields made with the key width options `ADS_UCHAR_KEY_SHORT`, `_LONG` and `_XLONG` (alone, among other fields, with unique, descending and compound options, on Char and Integer fields where they mean nothing, and refused with a bit outside the mask), rows written through them, a pack and a reindex, seeks, and the raw keys of every `AdsBuildRawKey100` option (`ADS_GET_PRIMARY_WEIGHT_LENGTH` among them) on each order (made by [`tools/ace/write/uchar_keys.py`](../tools/ace/write/uchar_keys.py)) |
| `nchar_expr` | ADI orders on expressions over NChar and NVarChar fields (`UPPER`, `LOWER`, `SUBSTR`, `LEFT`, `RIGHT`, the trims and pads, `+` with NChar, NVarChar and Char operands, `IIF`, `COALESCE`, `LEN`), made before and after the records, with the key width options, descending, unique and with a `FOR` condition, kept current through appends, changes and deletes, rebuilt by `AdsReindex` and pack, and sought and scoped (the original keeps Unicode text apart from 8-bit text by the expression; keys are 3 bytes a character and 6 more, text functions count characters, `LEN` UTF-16 units, a pad's fill is its first two units) (made by [`tools/ace/write/nchar_expr.py`](../tools/ace/write/nchar_expr.py)) |
| `nchar_expr_more` | ADI orders on expressions over NChar and NVarChar fields with `REPEAT` (a constant count), `DESCEND`, `VAL`, `SOUNDEX`, `BASE64ENCODE` and `COALESCE` with an 8-bit first argument, kept current and sought (`DESCEND` complements the sort key and its padding; `VAL` and `SOUNDEX` read the text as Windows-1252 with 0x1A for what does not map; `BASE64ENCODE` encodes the UTF-16LE bytes; every order of a Unicode table is sought by sort key) (made by [`tools/ace/write/nchar_expr_more.py`](../tools/ace/write/nchar_expr_more.py)) |

## DBF tables

`fixtures/dbf/getters.json` is what the original engine reports about DBF
tables, recorded by [`tools/ace/dbf/dbf.c`](../tools/ace/dbf/dbf.c) under
Wine on copies of the NTX, CDX and VFP fixtures (the list is in
[`run.sh`](../tools/ace/dbf/run.sh)): each table's record counts, memo block
size, record length, character type and last update; every getter,
`AdsIsNull`, `AdsIsEmpty`, `AdsGetFieldType`, `AdsGetFieldLength` and
`AdsGetFieldDecimals` on every field of every record and at EOF, with
`AdsIsRecordDeleted`; and the records a walk passes and the counts with
`AdsShowDeleted(FALSE)`. On the OEM table (`co.dbf`, opened with `ADS_OEM`)
only the text getters are called. Each call names its table; otherwise the
format is that of the field getters.

`fixtures/dbf/indexes.json` is what it reports about the NTX, CDX and IDX
fixtures' orders (those that open with their table, then those of the files
opened with `AdsOpenIndex`), recorded by
[`tools/ace/dbf/index.c`](../tools/ace/dbf/index.c)
([`index.sh`](../tools/ace/dbf/index.sh) lists the files): each order's
name, expression, condition, key length and type, flags and key counts;
the position (record, key number, `AdsExtractKey`'s key, BOF, EOF, found)
after every move of a walk to the end and back; and seeks of the first 40
distinct keys as strings and raw, of their first characters, of a key past
the last, and of numbers and dates given as strings and doubles.

## Data dictionaries

`fixtures/dictionary/dd.json` is what the original engine reports about the
`dictionary`, `psm` and `dictionary_props` dictionaries, recorded by
[`tools/ace/dd/dd.c`](../tools/ace/dd/dd.c) under Wine on copies of them
(see [`run.sh`](../tools/ace/dd/run.sh)), connected as `ADSSYS`:

- the return codes of connects as other users;
- every object `AdsDDFindFirstObject` lists of every type, under no parent
  and under each table;
- every property of the database and of each object through the getter of
  its type, into an 8,000-byte buffer, a 3-byte one and, for a numeric
  property, the size it asks for, or 4 and 2 bytes when it does not say
  (passwords left out);
- tables, views and other names opened by name, with what the table and its
  indexes report;
- the tables `AdsFindFirstTable` and `AdsFindFirstTable62` list.

Each call is `{"dd", "fn", "args", "rc"}`, with `"size"` (the buffer's),
`"len"` (the length written back) and `"value"` (the bytes written, as hex,
up to the first untouched byte) where the call has them. The data folder
shows as `<data>` in values, and lengths count it as 6 bytes.

`fixtures/dictionary/login.json` is what the original engine returns when a
connection logs in to the `dictionary` fixture (`ADSSYS` with password
`adssyspw`, the user `alice` with password `secret`), recorded by
[`tools/ace/dd/login.c`](../tools/ace/dd/login.c) under Wine (see
[`run_login.sh`](../tools/ace/dd/run_login.sh)). Each case is
`{"user", "password", "rc"}`, a null user or password being a NULL pointer:
right, wrong, empty and missing passwords, and the case of the user name.

`fixtures/dictionary/rights.json` is how the original engine enforces the
`dictionary_props` fixture's access rights (it verifies them), recorded by
[`tools/ace/dd/rights.c`](../tools/ace/dd/rights.c) under Wine (see
[`run_rights.sh`](../tools/ace/dd/run_rights.sh)). That dictionary has `ADSSYS`
(the administrator), `bob` (password `bobpw`) in group `readers`, with
`SELECT`/`INSERT` on `parent` to `readers` and `UPDATE(label)` on `parent` to
`bob`, and no grant on `child`. For each user it records the return code of
opening each table, reading its fields, column-permission queries, `SELECT`s and
(for `bob`) inserts/updates/deletes through SQL and the table API — the
operations whose outcome an access right decides.

`fixtures/dictionary/ri.json` is how the original engine enforces the
`dictionary_ri` fixture's referential-integrity rules, recorded by
[`tools/ace/dd/ri.c`](../tools/ace/dd/ri.c) under Wine (see
[`run_ri.sh`](../tools/ace/dd/run_ri.sh), which also builds the fixture itself
through the client API). It records the return code — and, for a check `SELECT`,
the resulting rows — of child inserts and updates with a present, absent and NULL
foreign key, and of parent deletes and key updates under each rule (RESTRICT,
CASCADE, SET NULL, SET DEFAULT), through both SQL and the table API.

`fixtures/dictionary/constraints.json` is how the original engine applies the
`dictionary_constraints` fixture's field defaults, field constraints (NOT NULL,
minimum, maximum, field messages), record validation and triggers on write,
recorded by [`tools/ace/dd/constraints.c`](../tools/ace/dd/constraints.c)
under Wine (see [`constraints.sh`](../tools/ace/dd/constraints.sh), which also
builds the fixture from [`constraints.sql`](../tools/ace/dd/constraints.sql)).
It runs the operations of [`constraints.ops`](../tools/ace/dd/constraints.ops)
in order on a fresh copy — table-API appends (with what the defaults put in the
new record), field sets, writes, updates and deletes, and SQL — and records
each step's return code and `AdsGetLastError` text, and the rows of each
`SELECT` (the triggers' log among them). It was recorded on the remote server
(the 12.0 evaluation server under Wine, `constraints.sh <work> <dll> remote
<share>`), whose per-record transactions take back what a failing trigger
wrote, through SQL and the table API alike. `constraints_local.json` is the
same on the local server, which has no transactions: there the log keeps the
rows of the BEFORE INSERT trigger that refused `id 99` (table API and SQL) and
of the BEFORE DELETE trigger that refused `delete k 2` — nothing else differs.
tflip replays it with `[transactions] mode = count`.

`fixtures/dictionary/rules.json` is the same probe (`constraints.c`, whose
`fprop` operation calls `AdsDDSetFieldProperty`) running
[`rules.ops`](../tools/ace/dd/rules.ops) on the `dictionary_rules` fixture,
built from [`rules.sql`](../tools/ace/dd/rules.sql) (see
[`rules.sh`](../tools/ace/dd/rules.sh)), on the local server: the order of a
record's constraints, its BEFORE triggers and its referential integrity
(RESTRICT, CASCADE, SET NULL and SET DEFAULT rules whose children have
constraints and triggers of their own), defaults, minimums and maximums that
are expressions of each type on fields of each type, and the defaults,
minimums and maximums `AdsDDSetFieldProperty` refuses for each field type
(memo, binary, logical and automatic fields among them). The triggers' log
keeps what a refused change's triggers wrote, the local server having no
transactions; tflip replays it with `[transactions] mode = count`.

`fixtures/dictionary/pubperm.json` is how the original engine grants and
revokes permissions on publications and subscriptions (and refuses them on
articles), recorded by [`tools/ace/dd/pubperm.c`](../tools/ace/dd/pubperm.c)
under Wine (see [`pubperm.sh`](../tools/ace/dd/pubperm.sh)): the return code and
masks of each `AdsDDGrantPermission`/`AdsDDRevokePermission`/
`AdsDDGetPermissions` call and dumps of `system.permissions`. The dictionaries
it leaves are kept in `fixtures/dictionary/ops/` (`permpub*`, `permsub*`,
`revkinds`, `revnone`, `colrev`, `colrev0`).

`fixtures/dictionary/viewlong/` is a dictionary the original engine made with
[`tools/ace/dd/ddwrite.c`](../tools/ace/dd/ddwrite.c) under Wine (see
[`ddwrite.sh`](../tools/ace/dd/ddwrite.sh)): an ADT table `t` (in `free/` as
it was before it was added) and views whose statements just fit the record's
property field (268 bytes) or go on into the `More Property` memo (269, 300,
1,000 and 3,000 bytes, the last with a comment).

`fixtures/dictionary/dbftables/` is the same for DBF tables, each added with
its index file: a FoxPro table `c` with a memo and a compound index, a Clipper
table `n` with an NTX index, a Visual FoxPro table `v` with a compound index
and a FoxPro table `c0` without an index (their files, unchanged by being
added, also in `free/`). `fixtures/sql/dbfdict.json` is what the original
engine does with it, recorded by `sql.c` from
[`tools/ace/sql/dbfdict.txt`](../tools/ace/sql/dbfdict.txt) on a copy
(`run.sh <work> <dll> dbfdict.txt`): the tables listed, opened by name, and
read and written through SQL.

`fixtures/dictionary/dbfcorners/` is the same for the corners of DBF tables
(`ddwrite.c`'s `dbfcorners`): OEM tables made and added with either character
type, Visual FoxPro tables with NULL-able fields, `ADS_DEFAULT` as the table
type, the character types each type takes, and index files of the wrong type
(the calls' return codes and texts are in the test, `ddl::dbf_corners`; the
files they were made on are in `free/`, and `t0` to `t17` are tables for the
character types).

`fixtures/sql/ddbf.json` is what the original engine does with `CREATE TABLE`
of Clipper, FoxPro and Visual FoxPro tables on a dictionary connection
(`run.sh <work> <dll> ddbf.txt`, from
[`tools/ace/sql/ddbf.txt`](../tools/ace/sql/ddbf.txt)): the tables join it, with
primary keys, field rules, NULL columns and OEM tables, and `DROP TABLE`;
`fixtures/sql/ddbf/` holds the dictionary and the files it leaves.
`fixtures/sql/dbfpk.json` is the same on a free connection
([`dbfpk.txt`](../tools/ace/sql/dbfpk.txt)): a Visual FoxPro table's primary
key and NULL columns, what the others refuse, and the files left in
`fixtures/sql/dbfpk/`.

`fixtures/sql/ddtables.json` is what the original engine does with `CREATE
TABLE` and `DROP TABLE` on a dictionary connection, recorded by `sql.c` from
[`tools/ace/sql/ddtables.txt`](../tools/ace/sql/ddtables.txt) in an empty
folder (`run.sh <work> <dll> ddtables.txt`): the script creates the
dictionary `ct/ct.add` (`ddcreate`), makes tables with field rules, memo
block sizes and primary keys, the refusals, drops tables and makes one in
their objects' slots, then runs the same on a free connection to the folder.
`fixtures/sql/ddtables/` holds the folder it leaves: the dictionary and every
table file, which tflip's replay compares byte for byte (the dictionary's
login secret aside, which the engine salts anew on every write).

`fixtures/sql/spt.json`, `sptdbf.json` and `sptdd.json` are the table system
procedures (`sp_PackTable`, `sp_ZapTable`, `sp_Reindex`, `sp_EncryptTable`,
`sp_DecryptTable`, `sp_GetTableEncryptionType`, `sp_GetTableSizeInfo`,
`sp_GetRecordCRC` and the `…Online` forms) on free ADT tables
([`spt.txt`](../tools/ace/sql/spt.txt)), on DBF tables
([`sptdbf.txt`](../tools/ace/sql/sptdbf.txt)) and on the tables of a
dictionary ([`sptdd.txt`](../tools/ace/sql/sptdd.txt)), each in an empty
folder (`run.sh <work> <dll> spt.txt`): their result sets, their arguments'
errors, the memo block and page sizes they take, and the files they leave in
`fixtures/sql/spt/`, `sptdbf/` and `sptdd/`, which the replay compares byte
for byte. (Encrypting DBF tables is not in them: tflip does not do it.)

`fixtures/sql/spm.json` is the management procedures (`sp_mgGet…`,
`sp_mgKillUser`, `sp_mgResetCommStats`, and the `sp_mgGetCrashDumpInfo`,
`sp_mgGetErrorLog` and `sp_mgSetConfigValue` the local server refuses) on a
free connection ([`spm.txt`](../tools/ace/sql/spm.txt)): their columns,
what they take and refuse, and the rows of those that do not depend on the
machine (the replay compares only the columns of the lists of what is open,
in use or connected). `spm1.json` and `spm1dd.json` are `sp_SetStatementLimit`,
`sp_SetRequestPriority`, `sp_CancelQuery` and `sp_IgnoreTableTransactions` on
free tables ([`spm1.txt`](../tools/ace/sql/spm1.txt)) and on a dictionary's
([`spm1dd.txt`](../tools/ace/sql/spm1dd.txt)), with the tables and the
dictionary they leave in `fixtures/sql/spm1/` and `spm1dd/`.

`fixtures/sql/spcat.json` and `spddcat.json` are the catalog procedures the
ODBC and JDBC drivers call (`sp_GetCatalogs`, `sp_GetSchemas`,
`sp_GetTableTypes`, `sp_GetTypeInfo`, `sp_SupportsConvert` over every pair of
the 21 types it knows, `sp_SupportsTxnIsolationLevel`,
`sp_RetrieveSystemProcProperty`, `sp_GetProcedures`,
`sp_GetProcedureColumns`) and `system.systemprocedures` on a free connection
([`spcat.txt`](../tools/ace/sql/spcat.txt)) and on a dictionary's, where a user
who is not the administrator runs the procedures
([`spddcat.txt`](../tools/ace/sql/spddcat.txt), the dictionary it leaves in
`fixtures/sql/spddcat/`). `spcoll.json` and `spcoll2.json` are
`sp_GetCollations` and `sp_GetCollationTable`
([`spcoll.txt`](../tools/ace/sql/spcoll.txt), `spcoll2.txt`: the numbers the
collations get as the engine loads them); they hold the SDK's `adscollate.adt`
and the ICU locales its engine lists, and the replay needs the file.
[`gen_catalog_data.py`](../tools/ace/sql/gen_catalog_data.py) makes the data
tflip ships for these from the fixtures. `spql.json` to `spql3.json`, `spql6.json`
and `spddql.json` to `spddql4.json` are query logging (`sp_EnableQueryLogging`,
`sp_GetQueryLoggingResults`, `sp_ViewQueryLogging`, `sp_DisableQueryLogging`)
on a free and on a dictionary connection
([`spql.txt`](../tools/ace/sql/spql.txt) and the others beside it, with
[`spddql.txt`](../tools/ace/sql/spddql.txt)): the log table's rows for each
kind of statement, the filters and options, what is refused; the replay
masks the times. `spql6/` holds the files of a log nothing was written to,
which the replay compares byte for byte. `spddql3.json` and `spddql4.json`
([`spddql3.txt`](../tools/ace/sql/spddql3.txt),
[`spddql4.txt`](../tools/ace/sql/spddql4.txt)) are the statements a trigger, a
procedure and a script run, which are rows of their own (their text as
written, unoptimized, the rows changed so far in the script, the end time),
with the filters. `spmg.json` and `spddmg.json` are `sp_GetUnicodeConverters`,
`sp_mgGetCacheInfo`, `sp_mgGetTableCacheInfo`, `sp_mgGetConfigMemory90` and
`sp_mgKillUser90` on a free connection and a dictionary's
([`spmg.txt`](../tools/ace/sql/spmg.txt),
[`spddmg.txt`](../tools/ace/sql/spddmg.txt)); `spmg.txt` sets `cap 300` so that
`sql.c` records all 222 rows of the converters.

`fixtures/sql/spdd*.json` are the dictionary system procedures, each recorded
from its [`tools/ace/sql/spdd*.txt`](../tools/ace/sql/) in an empty folder with
a dictionary of its own in `de/` (`run.sh <work> <dll> spddusers.txt`): users
and groups (`spddusers`), referential integrity (`spddri`, `spddrilong`),
links (`spddlinks`), `sp_GetSecurityInfo` (`spddsec`), the triggers
(`spddtrig`), the property procedures (`spdd?props`, by object kind), the
tables and index files (`spddtables`), `sp_RenameDDObject` (`spddrename`),
`AdsDDRenameObject` (`spddrenameapi`, which `sql.c` calls with `ddrename`),
publications and subscriptions renamed (`spddrenamerepl`, made with the `ddpub`,
`ddart` and `ddsub` commands of `sql.c`; `spddrenamerepl2` for the names the other kinds
of object share with them, its `de.ai` not compared),
`sp_MoveDDObjectFile` (`spddmove`, with `de/sub` and `de/sub2`) and
`sp_SetDDEncryptionType` (`spddenc`); `fixtures/sql/spdd*/` hold the
dictionaries they leave, compared byte for byte. `spddnum.json` is the
numbers the INTEGER and BOOL parameters take, and `spddparam.json` the
procedures called with `?` and `:name` parameters, which take their declared
types.

`fixtures/sql/selinto.json` is what the original engine does with `SELECT …
INTO table`, recorded from [`tools/ace/sql/selinto.txt`](../tools/ace/sql/selinto.txt)
in an empty folder (`run.sh <work> <dll> selinto.txt`): the free table made of
the result's columns of every type, of expressions, joins, `GROUP BY`, `TOP`,
`UNION` and procedures' results, under each table type of the statement, and
what it refuses; then the same on a dictionary connection (`dd/dd.add`).
`fixtures/sql/selinto/` holds the folder it leaves (the dictionary's folder in
`dd/`), whose tables tflip's replay compares byte for byte (the dictionary's
login secret aside).
`fixtures/sql/aep.json` is external procedures on the remote server
([`tools/ace/aep/`](../tools/ace/aep/): `aep.c`, a library of the version 2
interface, built as a 64-bit DLL linking the server's `ace64.dll`, and the
script `aep.txt`): results through `__input` and `__output`, a procedure in a
`FROM`, `Startup` and `Shutdown` and the connection id, the rows a library
says it affected, a table written in the caller's transaction, and the
failures. tflip replays it through the built client library, with the same
library as a shared object, on the local server
(`crates/tflip-ace/tests/aep.rs`) and in a `tflipd` it starts
(`crates/tflipd/tests/aep.rs`).
`fixtures/sql/aeprules.json` is what such a library may do with the handle it
is given and with connections of its own, on the original's local server (the
only one the probe could reach; `aeprules.c`, `aeprules.txt` and
`aeprules.sh` in `tools/ace/aep/`, a 32-bit DLL linking the SDK's `ace32.dll`
that `sql.exe` loads): `AdsBeginTransaction`, `AdsCommitTransaction`,
`AdsRollbackTransaction` and `BEGIN TRANSACTION`, `COMMIT`, `ROLLBACK` on the
passed handle (the local server lets them through, counting as everywhere
there, though the help says a version 2 library cannot), a connection the
library opens with `AdsConnect60` (type 1: asked for any type, the local
server connected to the evaluation server when it ran, a connection of type
2 with real transactions), a user with no rights (the local server checks
none), and a statement, table and cursor kept by one connection's procedure
and used by another's (no 5202 there). tflip replays it on the local server
(`crates/tflip-ace/tests/aeprules.rs`) and in a `tflipd`
(`crates/tflipd/tests/aeprules.rs`), which differs on purpose where the help
describes the original's server (5202, the connection type; the rights are
tested there with a script of its own).
`fixtures/sql/aeptrig.json` is triggers that are functions of a library
(`FUNCTION f IN LIBRARY`), on the remote server
([`tools/ace/aep/`](../tools/ace/aep/): `aeptrig.c`, a library with the trigger
prototype of the help, built as a 64-bit DLL and again with `-DPLAIN`, which
leaves out `GetInterfaceVersion`, `Startup` and `Shutdown`, and the script
`aeptrig.txt`; `run.sh aeptrig.txt aeptrig.c aeptrig.aep`): the arguments for
each event and timing, `__new` and `__old` read through the handle (and a
write to `__new` that changes nothing), what the change shows the library, the
failures through `__error` and the ones that are no failure (a return value, a
statement of the library that fails), the library's writes in the change's
transaction, `NO VALUES`, `NO MEMOS` and `NO TRANSACTION`, `ALTER TRIGGER`
between a script and a library, the priority among both, `ASSEMBLY`, and a
library as a database trigger. tflip replays it on the local server
(`crates/tflip-ace/tests/aeptrig.rs`) and in a `tflipd` it starts
(`crates/tflipd/tests/aeptrig.rs`).
`fixtures/sql/aeptables.json` is a library reaching `__input` and `__error`
through the table API, on the original's local server under Wine
([`tools/ace/aep/`](../tools/ace/aep/): `aeptables.c`, built as a 32-bit DLL
linking the SDK's `ace32.dll`, the script `aeptables.txt` and the runner
`aeptables.sh`, which copies the SDK folder so that the client and the
library load the same `ace32.dll`): the tables open as the temporary tables
`#__input` and `#__error` (or by their names with `ADS_TEMP_TABLE`), their
fields, positions and counts, writes to `__input`, and `__error` written with
`AdsAppendRecord`/`AdsSetLong`/`AdsWriteRecord` (the first record's number
and message fail the procedure, one posted by `AdsWriteRecord` or
`AdsCloseTable` and not one left appended). `__output` cannot be opened there
(7112), so no fixture says what a server does with it; a test of tflip's
own (`a_library_writes_output_through_the_table_api`) has the help's
behaviour. The library appends its trace to a table `log` too, because the
trace of a failed call is lost. Replayed in `crates/tflip-ace/tests/aeptables.rs`
and `crates/tflipd/tests/aeptables.rs`.
`fixtures/sql/execproc.json` is `EXECUTE PROCEDURE` as a table in `FROM`
([`execproc.txt`](../tools/ace/sql/execproc.txt)), on a copy of the `psm`
dictionary: user and system procedures, joins and conditions over their results,
their arguments and when they run.
`fixtures/sql/ddopts.json` and `fixtures/sql/ddoptsdict.json` are the
options of the DDL that runs: `CREATE INDEX … CANDIDATE | NOTDELETED | IN FILE
| PAGESIZE | ONLINE`, `CREATE TABLE … IGNORE TRANSACTIONS`, `ALTER TABLE …
IGNORE | RESPECT TRANSACTIONS`, `DROP PRIMARY KEY`, `ALTER COLUMN … DROP …`,
`ALTER ONLINE`, the VFP column options (`NULL`, `NOT NULL`, `NOCPTRANS`) and
`DROP TABLE … FROM DATABASE [NO_DELETE]`, with the refusals and which one
comes first, on a free connection
([`ddopts.txt`](../tools/ace/sql/ddopts.txt), run in an empty folder with `run.sh
<work> <dll> ddopts.txt`) and on a dictionary connection
([`ddoptsdict.txt`](../tools/ace/sql/ddoptsdict.txt), which also runs the
statements on a free connection to the dictionary's folder). `fixtures/sql/ddopts/`
and `fixtures/sql/ddoptsdict/` hold the folders they leave (the dictionary's
index `ct.ai` is not compared: the original keeps it up to date a record at a
time). `fixtures/sql/ddalter.json` is `ALTER TABLE` of a dictionary's tables
that changes columns (`ADD`, `ALTER` and `DROP COLUMN`, with `NOT NULL`,
`DEFAULT`, `MINIMUM`, `MAXIMUM` and `ERROR MESSAGE`, positions, memo block
sizes, `IGNORE TRANSACTIONS`): columns only named again renamed in place, any
other change a restructure that makes the Field objects anew, the orders in
the dictionary's index files following, field comments, a user without the right
to alter, and the refusals ([`ddalter.txt`](../tools/ace/sql/ddalter.txt), run in
an empty folder with `run.sh <work> <dll> ddalter.txt`); `fixtures/sql/ddalter/`
holds the folder it leaves (`ct.ai` is not compared as above).
`fixtures/sql/ddalterdbf.json` is the same on a dictionary's Clipper, FoxPro,
Visual FoxPro and ADT tables with orders: fields added, renamed and retyped,
which names restructure and which are written in the header alone (a Clipper
or FoxPro name is upper case, so a change of case is none), the Key objects'
expressions, lengths and key types following, `sp_CreateIndex90` and `CREATE
INDEX` making Key objects, and the refusals ([`ddalterdbf.txt`](../tools/ace/sql/ddalterdbf.txt),
`run.sh <work> <dll> ddalterdbf.txt` in an empty folder; `fixtures/sql/ddalterdbf/`
holds the folder, `cd.ai` not compared). `fixtures/sql/ddalterfail.json` is
an order the new types cannot build (a field with an order becomes a memo)
in a dictionary's tables: the table as it was, no index file, a new memo
file, two object ids per field and one per order used up
([`ddalterfail.txt`](../tools/ace/sql/ddalterfail.txt); `fixtures/sql/ddalterfail/`,
`cf.add` and `cf.ai` not compared, the original puts the objects back in other
places). `fixtures/sql/ddalterfree.json` is the same on free tables (the
table and its orders come back whole, but a Clipper table's memo file and
backup), with `MEMOBLOCKSIZE` of a Clipper table, the definitions errors show,
and which changes of names restructure
([`ddalterfree.txt`](../tools/ace/sql/ddalterfree.txt), run in an empty folder;
`fixtures/sql/ddalterfree/`).
`fixtures/sql/refuse.json` is what the original answers to the
statements tflip does not run (`FUNCTION` and `PACKAGE`) and to the `CREATE`,
`DROP` and `ALTER` words it has no statement of, `CACHE PREPARE ON | OFF |
DEFAULT` and the syntax errors of malformed `CREATE TABLE`, `CREATE INDEX`,
`ALTER TABLE`, `DROP TABLE` and `DROP INDEX` ([`refuse.txt`](../tools/ace/sql/refuse.txt), run in an empty folder
with `run.sh <work> <dll> refuse.txt`). `fixtures/sql/limits.json` is the limits of field lengths, names, index
columns and keys, and `ORDER BY` items on each table type, as SQL meets them
([`limits.txt`](../tools/ace/sql/limits.txt), made by
[`limits.py`](../tools/ace/sql/limits.py), run in an empty folder with `run.sh
<work> <dll> limits.txt`; the files it makes are not kept: `fixtures/write/limits`
has the byte-for-byte ones). `fixtures/sql/limits_procs.json` is how deep
procedures and triggers nest (7093) and how long a procedure's script is
([`limits_procs.txt`](../tools/ace/sql/limits_procs.txt), made by
[`limits_procs.py`](../tools/ace/sql/limits_procs.py), run with `run.sh <work>
<dll> limits_procs.txt`). `fixtures/sql/setcollation.json` is `SET ::conn.Collation` and `SET
::stmt.Collation`
([`setcollation.txt`](../tools/ace/sql/setcollation.txt), run on the folder of
`collation_dynamic.json` with `run_collation_dynamic.sh <work> <acesdk>
setcollation.txt`): the names they take and refuse, and that neither changes
what a client's statement opens its tables with.

`fixtures/sql/ddviews.json`, `ddprocs.json` and `dddatabase.json` are what the
original engine does with `CREATE VIEW`/`DROP VIEW`, `CREATE`/`ALTER`/`DROP
PROCEDURE` and `CREATE DATABASE`, recorded by `sql.c` from
[`ddviews.txt`](../tools/ace/sql/ddviews.txt),
[`ddprocs.txt`](../tools/ace/sql/ddprocs.txt) and
[`dddatabase.txt`](../tools/ace/sql/dddatabase.txt) in empty folders (`run.sh
<work> <dll> <script>`): the first two create a dictionary (`dv/dv.add`,
`dp/dp.add`), the last makes dictionaries in `dc/`. They record the views and
procedures with every kind of parameter, text and library path, `ALTER`
keeping a procedure's rights and `DROP` taking them (a user and grants made
with `dduser`), the refusals and their messages, objects made in the slots of
deleted ones, and `AdsDDAddView` on the same statements. The folders
`fixtures/sql/ddviews/`, `ddprocs/` and `dddatabase/` keep what the scripts
leave, which tflip's replay compares byte for byte (the secrets of the
Database object and of users and permissions, salted per dictionary, aside).

`fixtures/sql/ddslots.json` ([`ddslots.txt`](../tools/ace/sql/ddslots.txt), in
an empty folder, its dictionary in `ds/`, kept in `fixtures/sql/ddslots/`) is
what each kind of object takes as its slot when objects were deleted: filler
views are dropped one at a time and a user, group, member, link, trigger,
integrity rule, table added from a free one, index file, index and procedure
is added in their place. Dropping a user or group takes the rights it was
granted with it, after its own record.

`fixtures/sql/ddrights.json` ([`ddrights.txt`](../tools/ace/sql/ddrights.txt),
its dictionary in `dr/`, kept in `fixtures/sql/ddrights/`) is what a user who
is not the administrator may do to a dictionary: what each statement and
system procedure refuses with 5054 and says (views, procedures, tables,
indexes, triggers, users, groups, links, integrity rules), before any right
and with `GRANT CREATE ...`, `ALTER` and `DROP` to the user and to a group,
the rights the creator of an object holds on it, and what `system.triggers`
shows the user. `trigdbfire`, `trigdbopen`, `trigdblog`, `trigdblog2`,
`trigdblog3` and `trigself` keep their dictionaries (`fixtures/sql/trigdbfire/`
and so on) which the replay compares.
`fixtures/sql/ddvisible.json` ([`ddvisible.txt`](../tools/ace/sql/ddvisible.txt),
its dictionary in `vx/`, kept in `fixtures/sql/ddvisible/`) is what a user who is
not the administrator sees of a dictionary: the system tables and
`sp_GetTables`, `sp_GetColumns` and `sp_GetProcedures` for the administrator
and for users with only a right on one table, a column, a view, a procedure or
a function, with `ALTER` or with write rights, then once the dictionary has
stopped verifying access rights. Tables, columns, indexes, views, procedures,
functions and links are those the user holds a right on; a user without
`ALTER` on an object reads its description with the properties that say how it
is made left `NULL`; `system.dictionary` shows it the version, the paths, the
comment and the user-defined and full-text settings; the database's triggers
and the integrity rules are the administrator's alone. The script also gives
`sp_ModifyDatabase` the `LOGINS_DISABLED_MSG` and `FTS_NOISE_WORDS` names.

`fixtures/sql/ddlogins.json` ([`ddlogins.txt`](../tools/ace/sql/ddlogins.txt),
its dictionary in `lg/`, kept in `fixtures/sql/ddlogins/`) is what a connection to
a dictionary answers while its logins are disabled
(`sp_ModifyDatabase 'LOGINS_DISABLED'`): the administrator connects, any other
user with the right password is refused with 7109 and the database's message
after the text, one with a wrong password with 7078, and a user whose own logins
are disabled (`sp_ModifyUserProperty`) with 7113.

`fixtures/sql/ddtrigrights.json` ([`ddtrigrights.txt`](../tools/ace/sql/ddtrigrights.txt),
its dictionary in `gq/`, kept in `fixtures/sql/ddtrigrights/`) is which rights
let a user who is not the administrator run `sp_DisableTriggers` and
`sp_EnableTriggers`: `ALTER` on the table (or view) lets it for the table and
for its triggers, whether for the connection's user or all users (not the
`DB:Admin` the help names); the database's triggers for all users are the
administrator's alone (5054), and for the connection a user may enable them
but not disable them (5161).

`fixtures/sql/ddcolrights.json` ([`ddcolrights.txt`](../tools/ace/sql/ddcolrights.txt),
its dictionary in `cq/`, kept in `fixtures/sql/ddcolrights/`) is what a user who
may read and update only some columns of a table gets, at each
`TABLE_PERMISSION_LEVEL` (1, 2 and 3) and for a user with `SELECT` on the table
alone: a column it may not read reads as NULL in a select list and through the
table API; at level 2 a `WHERE`, `GROUP BY`, `ORDER BY` or join condition on one
is refused with 5171 (`<column> in table: <table>`), at level 1 it works on what
the column holds, and at level 3 the select list is refused too, every cursor is
static and `AdsOpenTable` fails with 7086; an `UPDATE` that sets a column the
user may not update is 5171 (`Update permission not granted to column`), and an
`INSERT` or `UPDATE` without the right on the table, as a `DELETE`, is 7087. The
open options of a cursor drop the permissions the user lacks, are read-only for
a user who may only read, and lack the bit `65536` at level 2 for a table with a
column it may not read.

`fixtures/sql/ddfieldlen.json` ([`ddfieldlen.txt`](../tools/ace/sql/ddfieldlen.txt),
`fixtures/sql/ddfieldlen/`) is the length and decimals a dictionary records
for a field of each type: a VARCHAR and a VARBINARY keep 2 more than they were
declared with, an NVARCHAR 1.

`fixtures/sql/ddfuncs.json` is what the original engine does with `CREATE`/
`ALTER`/`DROP FUNCTION` and `CREATE`/`DROP PACKAGE` (no `AdsDD*` call makes
them), recorded by `sql.c` from [`ddfuncs.txt`](../tools/ace/sql/ddfuncs.txt)
in an empty folder (`run.sh <work> <dll> ddfuncs.txt`), which creates the
dictionary `df/df.add`: the parameters and result types as the dictionary
spells them, descriptions, the names the scalar functions and other objects
take, functions in packages, `ALTER` (a new name, a description kept or
taken away, rights kept) and `DROP` (a package takes its functions and rights),
the refusals and their messages, objects made in the slots of deleted ones,
and a free connection. `fixtures/sql/ddfuncs/` keeps what it leaves, which
tflip's replay compares byte for byte (the secrets of the Database object and
of users and permissions aside).

`fixtures/dictionary/grants.json` is how the original engine runs SQL `GRANT`
and `REVOKE`, `AdsDDGrantPermission`/`AdsDDRevokePermission`/
`AdsDDGetPermissions` with object type 0, and the rights to create, recorded by
[`tools/ace/dd/grants.c`](../tools/ace/dd/grants.c) under Wine from the
scenarios in [`grants.txt`](../tools/ace/dd/grants.txt) (see
[`grants.sh`](../tools/ace/dd/grants.sh)): each step's return code, the error
text of a failure, the mask of a read and dumps of `system.permissions`. Each
scenario starts from an empty dictionary and builds its own users, groups,
tables (copies of `ops/t.adt`), views, procedures, links and publications
through the client API. The dictionaries ten of them leave are kept in
`fixtures/dictionary/ops/` as `grants_<scenario>.{add,am,ai}`.

`fixtures/dictionary/variants/` holds dictionaries (`.add` and `.am` only)
that differ from `base` in one property each, made by
[`tools/ace/dd/variants.c`](../tools/ace/dd/variants.c) under Wine from the
statements in [`variants.txt`](../tools/ace/dd/variants.txt): where a
dictionary keeps a property shows in the bytes that differ. `base` has one
table, `t`; the concurrency and web delta variants differ from
`table_rowversion` instead, which adds a table `r` with a ROWVERSION field.

## Error texts

`fixtures/errors/texts.json` holds the original engine's text for every
error code from 0 to 10000 it has one for, as `AdsGetErrorString` returns
it, recorded by [`tools/ace/errors/errors.c`](../tools/ace/errors/errors.c)
under Wine (Windows-1252, escaped as `\u00XX`). `tools/ace/errors/texts.py`
turns it into `crates/tflip-engine/src/ace_error/texts.rs`.

`fixtures/errors/handles.json` holds what calls that take one handle
answer when given a value that is no handle, 0, or a handle of the wrong
kind (a connection, a table, an index order, a statement): the return code
and `AdsGetLastError`'s code, recorded by
[`tools/ace/errors/handles.c`](../tools/ace/errors/handles.c) on a copy of
`adt/all_types`, each after a call that succeeds; both are replayed.

`fixtures/errors/handle_order.json` holds the last error, code and text,
that calls given wrong handles leave one after another, which depends on
the error the call before left (a table call's check keeps it, others
replace it): recorded by
[`tools/ace/errors/handle_order.c`](../tools/ace/errors/handle_order.c) on
a copy of `write/goto_deleted/before`, and replayed. The original hands
the statement made after a table closed that table's handle (`closed` in
the steps).

`fixtures/bookmarks/bookmarks.json` holds what the original does with
bookmarks taken through a table and through the five index orders of
`fixtures/adt/indexes/indexed.adt`: their length (8 through a table, 12 and
the key length through an order), their bytes (the record number, the key
length, the order's handle and the key), `AdsCompareBookmarks`' order (by
key and then record for two bookmarks of one order, reversed for a
descending one; by record alone otherwise), `AdsGotoBookmark60` and the
handle-sized `AdsGetBookmark` and `AdsGotoBookmark`, and the errors of bad
handles and NULL pointers. Recorded by
[`tools/ace/bookmarks/run.sh`](../tools/ace/bookmarks/run.sh) from its
`ops.txt` (the commands are listed in `bookmarks.c`) and replayed by
`bookmark.rs`; the handle of an order in a bookmark is written `{hN}`.

`fixtures/errors/param_n.json` holds what about 160 getters and setters of
tables, fields, orders, connections, statements, statement parameters and
the dictionary answer, and what `AdsGetLastError` says
after it, when a pointer of theirs is NULL (each in turn) and when an option is
one they refuse: the `Parameter N` of the text (N the number of the argument,
`AdsGetRelKeyPos`'s pointer being 3), which checks come before which (the
pointers before a field number, an option or a field's type for most, after
for `AdsGetIndexFilename`, `AdsGetKeyCount` and `AdsGetFieldType`), and the
calls that accept a NULL (`AdsGetLastAutoinc`, `AdsGetHandleType`) or leave
the last error alone (`AdsGetLastTableUpdate`, `AdsGetAOF` without a length).
Recorded by [`tools/ace/errors/param_n.sh`](../tools/ace/errors/param_n.sh)
from `param_n.txt` (the format of its lines is in `param_n.c`) and replayed by
`param_n.rs`. The dictionary getters' rows also hold why they refuse an
object or a property (5132, 5126, 5133, 5054).

`fixtures/errors/param_n_mg.json` holds the same for the `AdsMg*` calls, on
an `AdsMgConnect` handle of the original's remote server, a connection and
a local connection, with a handle that is none and `AdsGetHandleType` of an
`AdsMgConnect` handle. Recorded by
[`tools/ace/errors/param_n_mg.sh`](../tools/ace/errors/param_n_mg.sh) from
`param_n_mg.txt` and replayed by `param_n_mg.rs`, locally and on `tflipd`.

`fixtures/errors/record_count.json` holds what `AdsGetRecordCount` answers
on handles that are no table, for each filter option and with a NULL count
pointer. Recorded by
[`tools/ace/errors/record_count.c`](../tools/ace/errors/record_count.c).

`fixtures/errors/cursor_set.json` holds what the `AdsSet*` setters answer on
a SQL cursor: a live one (`SELECT *` or some columns of one table, and one
of a statement set read-only), and static ones (a join, a `GROUP BY`). Each
case has the return code, the last error, what `AdsWriteRecord` (or a move
to the last record) answered, the field's text afterwards, and whether the
table's files changed (they do on a live cursor, whether it is written, moved
off the record or just closed). Recorded by
[`tools/ace/errors/cursor_set.sh`](../tools/ace/errors/cursor_set.sh) (the
probe is `cursor_set.c`) and replayed.

## Expressions and filters

`fixtures/expr/compare.json` holds string comparisons on the
`charset_german` `ca` table under `ANSI_CHAR_SET=German`, from
`AdsEvalLogicalExpr` (`tools/ace/expr/compare.txt` with `probe.c`): each
byte compared with reference characters. `fixtures/expr/oem.json` holds the
text a client gives in filters and expressions about an OEM table,
converted to the table's code page (`tools/ace/oem`), replayed by
`oem_tables_convert_client_text`.

`fixtures/expr/` holds what the engine makes of expressions and filters on
the `adt` fixtures, recorded by the probes in
[`tools/ace/expr/`](../tools/ace/expr/) under Wine (`i686-w64-mingw32-gcc`;
the cases files are converted to Windows-1252 first):

- `expressions.json`, from `probe.c` and `cases.txt`: per expression, the
  result of `AdsIsExprValid` (`valid`, `valid_error`), the type from
  `AdsEvalTestExpr`, and on every record and at EOF the results of
  `AdsEvalLogicalExpr` (`L`: `T`/`F`), `AdsEvalNumericExpr` (`N`: `%.17g`)
  and `AdsEvalStringExpr` (`S`: hex), each `E<code>` when it fails. Volatile
  expressions (`DATE()`) have no records. A NULL numeric result is whatever
  the engine left in memory (a denormal such as `6.7492094464279322e-317`).
- `filters.json`, from `filters.c` and `filters.txt`: the result of each
  command (setting filters and AOFs, moves, seeks, `AdsLocate`,
  `AdsCustomizeAOF`) and the table's state after it.
- `keytype.json`, from `keytype.c` (see `run_keytype.sh`, which also builds the
  `expr_keys` fixture): for each expression order, `AdsGetKeyType` and
  `AdsGetKeyLength`, and the result of seeking a numeric expression order by a
  double key (whether found, and the record landed on).
- `case.json`, from `run_case.sh` on the `charset_german` `ca` table under
  `ANSI_CHAR_SET=German`: the `AdsEvalStringExpr` value of `UPPER(c)`, `LOWER(c)`,
  `UPPER(w)` and `LOWER(w)` on each record, to show how the static collation
  folds case.

Expressions and other text are Windows-1252 bytes written as U+0000–U+00FF.

## Seeks (fixtures/seek/)

From [`tools/ace/seek/`](../tools/ace/seek/) (`run.sh`; `cases.py` writes the
command lists, `seek.c` runs them) under Wine. Each file is a list of
`{"case", "out"}`: a command (`open`, `seek`/`wseek`/`rseek` soft, hard or
after, `last`/`wlast`, `scope`/`wscope`, `clear`, `walk`, `key`, with text as
hex of its Windows-1252 bytes, or of UTF-16LE for the `w` commands) and where
the table landed (the record, `E`/`B`/`BE`, `*` if found), the walk forward and
back, or the key.

- `collation.json`: the `charset_german` tables under `ANSI_CHAR_SET=German`
  and `OEM_CHAR_SET=GERMAN`: seeks, seek-lasts and scopes in the `ca` table's
  ADT, NTX and CDX orders and the `co` (OEM) table's NTX and CDX ones.
- `oem.json`: the same OEM seeks in the `charset_default` `co` tables, under the
  default collations, for the client's text converted to the OEM code page.
- `nchar.json`: the `adi_keys` table's NChar and NVarChar orders (ICU sort
  keys, `:en_US`): their keys, and seeks and scopes by ANSI and UTF-16 text.
- `nkeys.txt`, from `run_nkeys.sh` (`nkeys.c`, `nkeys.py`): 2,000 random strings
  of the characters `crates/tflip-storage/src/icu_root.txt` knows, as UTF-16LE
  hex, each with the key `AdsExtractKey` gave for it in an NChar(12) order (42
  bytes). The same script works out `icu_root.txt` itself from the engine's
  keys of every BMP character.
- `crates/tflip-storage/src/icu_case.txt`, from `run_ucase.sh` (`ucase.c`,
  `ucase.py`) rather than a fixture: what the engine's `UPPER` and `LOWER` make
  of every BMP and first-plane character of NChar text, worked out from its
  keys of `UPPER(n)` and `LOWER(n)` (about 2,050 characters change; it is
  Unicode 5.1's mapping a character at a time, so ß and a final sigma stay).

## Code pages

`fixtures/codepage/setfromcodepage.json`, from
[`tools/ace/codepage/`](../tools/ace/codepage/) under Wine, records how the
engine's `AdsSetStringFromCodePage` converts each byte. For every single-byte
code page the engine accepts, it lists the UTF-16 unit each byte `0x00..0xFF`
produced, read back from a Unicode field with `AdsGetFieldW`. These are the
engine's own mappings (they differ from Windows' standard tables); byte `0x00`
reads back as `0x20`, a stored NUL shown as a space, where its true conversion
is U+0000.

`fixtures/codepage/dbcs.json`, from [`tools/ace/codepage/codepage2.c`](../tools/ace/codepage/),
is the same for the four double-byte code pages: each byte `0x00..0xFF` set on its own, the
lead bytes (those that take two bytes) and the conversions of their pairs. `tflip-ace`
bakes its tables from it (`tools/ace/codepage/gen_dbcs.py`).

## Static collations by host locale

`fixtures/collation/sysdefault.json`, from
[`tools/ace/collation/sysdefault.py`](../tools/ace/collation/) (running
`sysdefault.c` under Wine with `LC_ALL` set to each locale), records which
static collations the engine uses with `ANSI_CHAR_SET` and `OEM_CHAR_SET`
empty. Per locale (`runs`): the Windows locale Wine made of it, and for the
bytes 1–255, as hex, a DBF's NTX order opened ANSI (`ansi`) and OEM (`oem`),
each byte's ADI key (`weights`), what `UPPER(c)`/`LOWER(c)` evaluate to on an
ADT table (`upper`, `lower`), and the collation CRC the ADI file records
(`crc`). `native` holds the same probe run on Windows 10 with the SDK's
`ace32.dll`, the user and system locales set apart (and, last, the user
locale's `sLanguage` alone set to `ENU`); there the NTX orders were built with
a null tag, which changes nothing under Wine.

## Collation mismatch options (fixtures/collation/mismatch.json)

`fixtures/collation/mismatch.json`, from
[`tools/ace/collation/run_mismatch.sh`](../tools/ace/collation/run_mismatch.sh)
(running `mismatch.c` under Wine), records what `AdsOpenTable` does when the
table's ADI index was built under another collation than the connection's,
with `ADS_REINDEX_ON_COLLATION_MISMATCH` (`0x100`) and
`ADS_IGNORE_COLLATION_MISMATCH` (`0x200`) among the open options: the
`charset_german` `ca` table (built under German) opened under the default
collation with each option, with both, shared and read-only, and with another
connection holding the table open; a table whose index file has a custom order
(`fixtures/collation/mismatch/custom`, made by `mismatch_custom.c` under
German); and the `collation_dynamic` `cze` table opened with other dynamic
collations. Per case: the open's code and error text, the orders, the table's
and first order's collations, a soft seek and a walk of the first order, and the
CRC and size of the index file after (it is rewritten by the reindex).
`fixtures/collation/mismatch/ca_reindexed.adi` is that index file after the
reindex on opening, which tflip's must equal byte for byte.

## `AdsConnect101`'s options (fixtures/config/connect101.json)

`fixtures/config/connect101.json` and `connect101_values.json`, from
[`tools/ace/config/run_connect101.sh`](../tools/ace/config/run_connect101.sh)
(running `connect101.c` under Wine on the tables of `fixtures/config/connect101/`,
each with its second record deleted), record what the options of a connect
string do: which keys and values `AdsConnect101` accepts (the first file the
keys with a table of each type opened through `AdsOpenTable101`, the second the
values of each key, nothing opened), the connection's settings afterwards
(`AdsGetCollation`, `AdsGetDateFormat60`, `AdsGetExact22`) and how
`AdsOpenTable101` opened each table (its type, character set, lock type and open
options, whether it could be appended to or opened exclusively elsewhere).

## Open-table caches

`fixtures/cache/remote.txt` and `fixtures/cache/local.txt` are what
[`tools/ace/cache/cache.c`](../tools/ace/cache/) printed with the original
32-bit `ace32.dll` (12.0 SDK) under Wine: against the evaluation server
(under Wine, setup in `LOCAL.md`, over TCP) and on the local server. The
probe sets `AdsCacheOpenTables`/`AdsCacheOpenCursors`, opens and closes tables
and runs `SELECT`s on several connections, and after each step lists what is
open (`AdsMgGetOpenTables`, through a connection of its own), tries an
exclusive open from another connection, drops a table, and so on; one line per
call with its answer (the error text cut to 90 characters). `tflip-ace`'s
`open_cache` test replays the same calls through `tflipd` and compares the
answers and the listings with `remote.txt` (`ads_err.adt`, the server's error
log, aside). `local.txt` shows that the original's local server caches the
same way (tflip's does not; see `docs/KNOWN_ISSUES.md`).

## Online backup and restore (fixtures/backup/)

What the ADS 12 evaluation server (remote) answers and writes for
`sp_BackupDatabase`, `sp_BackupFreeTables`, `sp_RestoreDatabase` and
`sp_RestoreFreeTables`, and what the local server answers (5042 for all four):
one folder per scenario with `result.json` (the cursor's columns and rows, the
errors), `errlog.txt` (what the server logged) and `tree/` (the source, the
backup images and the restored folders, byte for byte). Made by
`tools/ace/backup/run.sh <scenario> [local]` (scripts in
`tools/ace/backup/scripts/`, on top of `tools/ace/events/ev.c`). The
findings, row by row and byte by byte, are in
[backup/README.md](backup/README.md).

## Reproducibility

Regenerating gives byte-identical files except:

- **ModTime** fields hold the time of the last write (`modtime` fixture only).
- **`.add` files** contain encrypted passwords and rights that differ on every
  run, and a 20-byte value that the engine rewrites on every connection.

## Limitations

- **AES encryption** needs OpenSSL DLLs that no installer ships; the AES
  fixtures were built separately (below).
- **Conditional indexes** need `AdsCreateIndex61`; SQL `CREATE INDEX` cannot
  create them. Separate index files (`IN FILE`) only work in a dictionary.
- **DBF field names** are limited to 10 characters.
- The sample data is written by 12.0.0.2 only; files from other ADS versions
  may differ in details.

## AES encryption (fixtures/aes/)

`fixtures/aes/` holds two AES dictionaries built by the engine: `aes256/`
(password `ddsecret`: `t`, two plain rows; `m`, Memo and Blob values in slab
chunks and a 36-block extent, an index, a row added after encryption; `big`,
400 rows with three orders, one descending, deep enough for interior pages) and
`aes128/` (password `another pw`: `s`, with a memo). Each table has the
engine's decrypted JSON dump; `passwords.json` lists the dictionary passwords.

`fixtures/aes/free/` holds two *free* (non-dictionary) AES tables the engine
makes on a connection with `EncryptionType=AES128` (fields `id,Integer;
name,Character,12`, no rows): `free.adt`, the finished table after
`AdsEnableEncryption("mypass")` + `AdsEncryptTable` (its 32-byte salt, 4-byte
nonce, 8-byte IV counter, descriptor IV and PBKDF2 verifier are random, so it is
a byte-for-byte golden only with those values injected); and `unarmed.adt`, the
table right after `AdsCreateTable` on the AES connection but before enabling —
the full AES structure (data pushed to 0x640 by the 600-byte crypto extension,
the `0x14`/`0x164` markers, fields at record offset 13) with its crypto block,
verifier and encryption type still all zero and only the `CREATED` flag set.
Being all-zero in the random parts, `unarmed.adt` is deterministic (it has a
third `note,Memo` field, so it also carries the `MEMO` flag). Both were captured
with the OpenSSL-shipping patched engine under Wine (see `LOCAL.md`).

`fixtures/aes/dict/` holds AES dictionaries made with known secrets by
`tools/ace/crypto/aesdict.sh` (dictionary password `ddpw` throughout; see the
crypto FINDINGS, "AES dictionaries"): `a2/` (AES-128, no ADSSYS password),
`a3/` (AES-128) and `a4/` (AES-256), both with the ADSSYS password `admpw`,
keep their records in the clear; `e3/` (AES-128, `admpw`) is created with
`ENCRYPT TRUE` and enciphers them. Each has a user `u1` (`userpw`) in a group
`g1` and grants on a table `t`; `a2` and `a3` a user `u2` with no password,
and `e3` a second table `t2` created after `ENCRYPT_NEW_TABLE`. `a3`'s `t`
was encrypted after it was made, `e3`'s `t` never was. `aesdict.json`,
`aesdict2.json` and `aesdict3.json` are what the probe's steps returned,
`aesdict3.json` in a process of its own: the connections that open the
dictionaries and those refused (7068 without the dictionary password, 7060
with a wrong one).

`fixtures/aes/dictwrite/` is what the original writes to AES dictionaries
(`tools/ace/crypto/aeswrite.sh`; dictionary password `ddpw`, no ADSSYS
password): `snap/<step>/` holds the files of `c` (AES-128, records in the
clear), `e` (AES-128) and `f` (AES-256, both enciphering them) after each
script of `tools/ace/crypto/aeswrite/` — created, a table, a user and a group
(with descriptions) and the user in it, a grant, a long comment, a view, the
user dropped, a table made after `ENCRYPT_NEW_TABLE` and filled, tables
encrypted (`sp_EncryptTable`, `TABLE_ENCRYPTION`) and decrypted, one with a
memo and an index, `ENCRYPT_INDEXES` turned on and off, `AdsDDCreate101` of
AES dictionaries `h`, `i` (and the half-made `j`, `k` of its refusals), and
`CREATE INDEX`, `DROP INDEX`, `sp_Reindex`, `sp_PackTable` and
`sp_ZapTable` on a table whose index is enciphered (`16_index_create` to
`21_index_clear`), and `AdsDDCreate` with `usEncrypt` 1 (`22_rc4_create`: an RC4
dictionary `m`, which enciphers its records, its index and the memo values of
its objects, each value at the slot of the record that refers to it, and `n`
made with another flag), and what `sp_SetDDEncryptionType` refuses before it
converts (`23_convert_setup`, `24_convert_refused`) —
and `<step>.json` what its steps returned. tflip replays the scripts, on
copies of the original's last snapshot each and from its own `CREATE
DATABASE`, and compares the files deciphered and what each step fails with
(`dictionary_write::aes_replay`; `AES_ONLY=10` replays one script,
`AES_KEEP=1` keeps the files).

`fixtures/aes/convert/` is what `sp_SetDDEncryptionType` does to a dictionary
of every layout (`tools/ace/crypto/aesconvert.sh`; ADSSYS password `adm`,
dictionary password `ddpw`): `before/<letter>/d.{add,ai,am}` is the dictionary
each `tools/ace/crypto/aesconvert/a_<letter>.txt` makes (`P` plain, `R` RC4,
`C` AES-128 keeping its records in the clear, `E` AES-128 and `F` AES-256
enciphering them, `T` a plain one with an encrypted table that a conversion
refuses, `X` a plain one with a referential integrity rule, a link and rights of
every kind), and `after/c_<letter>_<type>_<last argument>/` the files that the
conversion `c_….txt` makes of a copy of it leaves, the `.BAK` copies of its
`.add` and `.am` among them when it writes the dictionary anew. tflip replays the
conversions on copies of `before` and compares the files deciphered
(`dictionary_write::aes_convert`; `AES_ONLY=E_AES` replays the scripts whose
names hold it).

`fixtures/write/encryption/stmtpw.json` is what `tools/ace/crypto/stmtpw.sh`
saw of a free RC4 table opened through SQL statements (its folder written
`<data>`): refused with 5098 without its password and 7060 with a wrong one,
read, written and dropped with the one `AdsStmtSetTablePassword` gave (by the
table's name in any case, not with an extension or a path), and tables
`AdsStmtEnableEncryption` leaves as they are. tflip replays its script,
`tools/ace/crypto/stmtpw.txt`.
