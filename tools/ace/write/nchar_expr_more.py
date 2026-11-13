#!/usr/bin/env python3
"""Writes fixtures/write/nchar_expr_more/{setup,ops}.txt: what nchar_expr.py
left out, orders on expressions of NChar and NVarChar fields with REPEAT,
DESCEND, VAL, SOUNDEX, BASE64ENCODE, and COALESCE with an 8-bit
first branch, made before and after the records, kept current through
appends, changes and deletes, rebuilt by a reindex and a pack, and sought,
for tools/ace/write/run.sh.

    tools/ace/write/nchar_expr_more.py fixtures/write/nchar_expr_more
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from nchar_expr import ASTRAL, TEXTS, fit, wide  # noqa: E402

# No empty NVarChar value: the original's BASE64ENCODE of one leaves the
# previous key's bytes in the key. The table nr is for REPEAT, which the
# original fails to write (4004) for a NULL NChar value.
SETUP = """\
# ADT tables with NChar, NVarChar and Char fields (ops create the orders).
sql CREATE TABLE ne (id INTEGER, nc NCHAR(8), nv NVARCHAR(10), c CHAR(8), nc2 NCHAR(3))
sql INSERT INTO ne VALUES (1, 'abc', 'Zebra', 'x', 'q')
sql INSERT INTO ne VALUES (2, 'été', 'apple', 'Yy', 'ré')
sql INSERT INTO ne VALUES (3, 'ÉTÉ', ' Ömega', 'z', 'Q')
sql INSERT INTO ne VALUES (4, 'éééé', 'ÉÉÉÉÉÉÉ', 'w', NULL)
sql INSERT INTO ne VALUES (5, 'ABCD', 'abcdefg', 'v ', '')
sql INSERT INTO ne VALUES (6, NULL, NULL, NULL, NULL)
sql INSERT INTO ne VALUES (7, 'Ab', 'Straße', 'Äb', 'ab')
sql INSERT INTO ne VALUES (8, '  pad', 'tail  ', 'Zz', 'zz')
sql INSERT INTO ne VALUES (9, 'abcdefgh', 'abcdefghij', 'abcdefgh', 'abc')
sql INSERT INTO ne VALUES (10, '', 'x', '', '')
sql INSERT INTO ne VALUES (11, '123', '12.5', '42', '7')
sql INSERT INTO ne VALUES (12, '-7.25', ' 8abc', '1e2', '-')
sql CREATE TABLE nr (id INTEGER, nc NCHAR(8), nv NVARCHAR(10), c CHAR(8), nc2 NCHAR(3))
sql INSERT INTO nr VALUES (1, 'abc', 'Zebra', 'x', 'q')
sql INSERT INTO nr VALUES (2, 'été', 'apple', 'Yy', 'ré')
sql INSERT INTO nr VALUES (3, 'ÉTÉ', ' Ömega', 'z', 'Q')
sql INSERT INTO nr VALUES (4, 'éééé', 'ÉÉÉÉÉÉÉ', 'w', 'q')
sql INSERT INTO nr VALUES (5, 'ABCD', 'abcdefg', 'v ', '')
sql INSERT INTO nr VALUES (6, 'Ab', 'Straße', 'Äb', 'ab')
sql INSERT INTO nr VALUES (7, '', 'x', '', ' ')
sql CREATE TABLE nb (id INTEGER, nc NCHAR(8), nv NVARCHAR(10), c CHAR(8), nc2 NCHAR(3))
sql INSERT INTO nb VALUES (1, 'abc', 'Zebra', 'x', 'q')
sql INSERT INTO nb VALUES (2, 'été', 'apple', 'Yy', 'ré')
sql INSERT INTO nb VALUES (3, NULL, NULL, NULL, NULL)
sql INSERT INTO nb VALUES (4, 'ÉTÉ', ' Ömega', 'z', 'Q')
sql INSERT INTO nb VALUES (5, 'abcdefgh', 'abcdefghij', 'abcdefgh', 'abc')
sql CREATE TABLE ns (id INTEGER, nc NCHAR(8), nv NVARCHAR(10), c CHAR(8))
"""

# On the rows the table starts with.
FIRST = [
    "DESCEND(nc)", "DESCEND(nv)", "DESCEND(nc2)", "DESCEND(UPPER(nc))", "DESCEND(nc+nv)",
    "DESCEND(LEN(nc))", "DESCEND(SUBSTR(nc,2,3))", "DESCEND(COALESCE(nc,nv))",
    "VAL(nc)", "VAL(nv)", "VAL(nc2)", "VAL(UPPER(nc))",
    "SOUNDEX(nc)", "SOUNDEX(nv)", "SOUNDEX(nc2)", "SOUNDEX(UPPER(nc))",
    "COALESCE(c,nc)", "COALESCE(c,nv)", "COALESCE(c,nc,nv)", "COALESCE('zz',nc)", "COALESCE(SUBSTR(c,1,2),nc)",
    "COALESCE(STR(id,3),nc)", "COALESCE(c,'zz')", "COALESCE(UPPER(c),nc)",
    "nc+NULL",
]
# After the rows are in.
LATER = [
    "DESCEND(PADL(nc,12))", "DESCEND(TRIM(nv))",
    "DESCEND(nc+c)", "DESCEND(IIF(id>2,nc,nv))", "DESCEND(LOWER(nv))",
    "VAL(SUBSTR(nc,2,3))", "VAL(nc)+VAL(nv)", "VAL(nv)*2",
    "SOUNDEX(SUBSTR(nc,2,3))", "SOUNDEX(nc)+nv", "SOUNDEX(nc)+SOUNDEX(nv)",
    "COALESCE(c,nc)+nv", "COALESCE(c,nc2,nv)", "COALESCE(STR(id,3),nv)", "COALESCE(c+c,nc)",
    "COALESCE(c,nc)+c", "UPPER(COALESCE(c,nc))", "LOWER(COALESCE('zz',nc))", "COALESCE(nc2,nc)",
    "TRIM(COALESCE(c,nc))",
]
# BASE64ENCODE counts a supplementary character as two, which a text function
# does not: the original fails 5005 when the encoding outgrows the key, so
# the table nb has no such text until the end.
BASE64 = [
    "BASE64ENCODE(nc)", "BASE64ENCODE(nv)", "BASE64ENCODE(nc2)", "BASE64ENCODE(UPPER(nc))",
    "BASE64ENCODE(nc+c)", "BASE64ENCODE(c)", "BASE64ENCODE(SUBSTR(nc,2,3))",
]
BASE64_LATER = [
    "BASE64ENCODE(nc)+nc", "BASE64ENCODE(nc+nv)", "BASE64ENCODE(PADL(nc,12))", "BASE64ENCODE(nc)+STR(id,3)",
    "BASE64ENCODE(LOWER(nv))", "BASE64ENCODE(nc)+BASE64ENCODE(nv)", "BASE64ENCODE(TRIM(nv))",
]
# The other tables' orders.
REPEATS = [
    "REPEAT(nc,2)", "REPEAT(nv,3)", "REPEAT(nc,1)", "REPEAT(nc,0)", "REPEAT(nc,-1)", "REPEAT(c,2)",
    "REPEAT(nc+c,2)", "REPEAT(nc,2)+nv", "REPEAT(UPPER(nc),2)", "UPPER(REPEAT(nc,2))",
    "REPEAT(nc2,40)", "REPEAT(nc,2.7)", "REPEAT(nc,3)+REPEAT(nv,2)", "REPEAT(SUBSTR(nc,2,3),3)",
    "REPEAT(LEFT(nc,2),4)", "REPEAT(PADL(nc,4),2)", "STR(id,3)+REPEAT(nv,2)",
    "REPEAT(nc,2)+REPEAT(nc2,2)", "DESCEND(REPEAT(nc,2))", "BASE64ENCODE(REPEAT(nc,2))",
    "COALESCE(REPEAT(c,2),nc)", "COALESCE(c,REPEAT(nc,2))",
]
# Their keys are numbers.
NUMBERS = {"VAL(nc)", "VAL(nv)", "VAL(nc2)", "VAL(UPPER(nc))",
           "DESCEND(LEN(nc))",
           "VAL(SUBSTR(nc,2,3))", "VAL(nc)+VAL(nv)", "VAL(nv)*2"}
SHORT, LONG, XLONG = 0x20000000, 0x40000000, 0x60000000
OPTIONS = [
    ("des_l", "DESCEND(nc)", LONG), ("des_d", "DESCEND(nv)", 8),
    ("coa_s", "COALESCE(c,nc)", SHORT), ("coa_x", "COALESCE(c,nc)", XLONG),
    ("coa_u", "COALESCE(c,nc)+STR(id,3)", 1),
]
SEEKS = {"DESCEND(nc)": ["abc", "ÉTÉ", "abc     "],
         "COALESCE(c,nc)": ["x", "zz", "Äb"],
         "SOUNDEX(nc)": ["A120", "T000", "a"],
         "BASE64ENCODE(nc)": ["YQBiAGMAIAAgACAAIAAgAA=="]}
REPEAT_SEEKS = {"REPEAT(nc,2)": ["abc", "abc     abc", "été     été"]}


def row(n, nc, nv, c, nc2=None):
    ops = ["append", f"set id long {n}", "set nc stringu " + wide(nc), "set nv stringu " + wide(nv),
           "set c string " + c]
    if nc2 is not None:
        ops.append("set nc2 stringu " + wide(nc2))
    return ops + ["write"]


def main(out):
    ops = ["# Orders on expressions of NChar and NVarChar fields, the rest.", "open ne.adt exclusive"]
    for i, expr in enumerate(FIRST):
        ops.append(f"createindex - o{i} 0 0 {expr}")
    ops += ["# Records written through them."]
    ops += row(13, "Привет", "Ωmega  ", "Ab", "é")
    texts = TEXTS + ASTRAL
    for n, text in enumerate(texts):
        ops += row(14 + n, text, fit(texts[(n + 5) % len(texts)], 10), ["ab", "Ab", " x", "Äb", "zz"][n % 5],
                   fit(texts[(n + 3) % len(texts)], 3))
    for n, (nc, nv) in enumerate([("12３", "１２３"), ("12abc", "3.5e2"), ("0x1F", "  9"), ("5", "-.5")]):
        ops += row(80 + n, nc, nv, "77")
    ops += ["append", "set id long 99", "set nc null", "set nv null", "set c null", "write",
            "# Changes and deletes.", "goto 1", "set nc stringu " + wide("ЯЖЁ"), "write",
            "goto 4", "set nv stringu " + wide("zebra"), "write", "goto 6", "delete", "write",
            "goto 10", "set nc null", "write", "goto 15", "delete", "write"]
    for i, expr in enumerate(LATER):
        ops.append(f"createindex - p{i} 0 0 {expr}")
    for i, (tag, expr, options) in enumerate(OPTIONS):
        ops.append(f"createindex - {tag} {options} 0 {expr}")
    ops += ["createindex ne2.adi des_for 0 512 DESCEND(nc) FOR id < 8",
            "createindex ne2.adi coa_for 0 512 COALESCE(c,nc) FOR id < 8",
            "# More records, with the orders there."]
    ops += row(70, "Ŝŭŝ", "ĥŭ", "Ab") + row(71, "        ", "  ", "x")
    ops += ["close", "open ne.adt exclusive", "reindex", "pack", "close"]
    ops += ["open nb.adt exclusive"]
    for i, expr in enumerate(BASE64):
        ops.append(f"createindex - b{i} 0 0 {expr}")
    for n, text in enumerate(TEXTS):
        ops += row(10 + n, text, fit(TEXTS[(n + 5) % len(TEXTS)], 10), ["ab", "Ab", " x", "Äb", "zz"][n % 5],
                   fit(TEXTS[(n + 3) % len(TEXTS)], 3))
    ops += ["append", "set id long 99", "set nc null", "set nv null", "set c null", "write",
            "goto 1", "set nc stringu " + wide("ЯЖЁ"), "write", "goto 4", "delete", "write"]
    for i, expr in enumerate(BASE64_LATER):
        ops.append(f"createindex - c{i} 0 0 {expr}")
    ops += ["close", "open nb.adt exclusive", "reindex", "pack"]
    # Text of supplementary characters outgrows the keys of the encodings.
    ops += ["createindex - bs 0 0 BASE64ENCODE(SUBSTR(nc,2,3))"]
    ops += row(60, "a🙂b", "x", "ab", "y")
    ops += ["close", "open nb.adt shared"]
    for tag in [f"b{i}" for i in range(len(BASE64))] + [f"c{i}" for i in range(len(BASE64_LATER))]:
        ops += [f"order {tag}", "rows", "seek 1 a", "seek 2 YQ"]
    ops += ["order b0", "seek 1 YQBiAGMAIAAgACAAIAAgAA==", "close"]
    # REPEAT, on a table whose NChar values are never NULL.
    ops += ["open nr.adt exclusive"]
    for i, expr in enumerate(REPEATS):
        ops.append(f"createindex - q{i} 0 0 {expr}")
    ops += ["createindex nr2.adi rep_for 0 512 REPEAT(nc,2) FOR id > 4",
            "createindex - rep_s %d 0 REPEAT(nc,2)" % SHORT, "createindex - rep_x %d 0 REPEAT(nc,2)" % XLONG,
            "createindex - rep_d 8 0 REPEAT(nc,2)"]
    for n, text in enumerate(texts):
        ops += row(10 + n, text, fit(texts[(n + 5) % len(texts)], 10), ["ab", "Ab", " x", "Äb", "zz"][n % 5],
                   fit(texts[(n + 3) % len(texts)], 3) or "x")
    ops += ["# Changes and deletes.", "goto 1", "set nc stringu " + wide("ЯЖЁ"), "write",
            "goto 4", "set nv stringu " + wide("zebra"), "write", "goto 6", "delete", "write", "goto 12", "delete", "write", "close",
            "open nr.adt exclusive", "reindex", "pack", "close"]
    ops += ["open ns.adt exclusive"]
    for tag, expr in [("a", "SOUNDEX(nc)"), ("b", "DESCEND(nc)"), ("c", "COALESCE(c,nc)"),
                      ("d", "COALESCE(nc,nv)"), ("e", "DESCEND(nv)"), ("f", "COALESCE(c,nv)"),
                      ("g", "VAL(nc)"), ("h", "VAL(nv)"), ("i", "SOUNDEX(nv)")]:
        ops.append(f"createindex - {tag} 0 0 {expr}")
    for n, text in enumerate(ASTRAL + ["a", "b😀"]):
        ops += row(n + 1, text, ASTRAL[(n + 2) % len(ASTRAL)], "Zz")
    ops += ["close", "open ns.adt exclusive", "reindex", "close"]
    ops += ["open ne.adt shared", "openindex ne2.adi"]
    tags = {expr: f"o{i}" for i, expr in enumerate(FIRST)}
    tags.update({expr: f"p{i}" for i, expr in enumerate(LATER)})
    for expr, tag in tags.items():
        ops += [f"order {tag}", "rows"]
        if expr in NUMBERS:
            continue
        for text in SEEKS.get(expr, ["a", "É", "ab"]):
            for kind in (1, 2, 4):
                ops.append(f"seek {kind} {text}")
        if expr in SEEKS:
            ops += [f"scope 1 {SEEKS[expr][0]}", f"scope 2 {SEEKS[expr][-1]}z", "rows",
                    "getscope 1", "getscope 2", "clearscope 1", "clearscope 2"]
    for tag, _, _ in OPTIONS:
        ops += [f"order {tag}", "rows", "seek 1 ab", "seek 2 É"]
    ops += ["order des_for", "rows", "order coa_for", "rows", "close"]
    ops += ["open nr.adt shared", "openindex nr2.adi"]
    for i, expr in enumerate(REPEATS):
        ops += [f"order q{i}", "rows"]
        for text in REPEAT_SEEKS.get(expr, ["a", "É"]):
            ops += [f"seek {kind} {text}" for kind in (1, 2, 4)]
    for tag in ("rep_s", "rep_x", "rep_d", "rep_for"):
        ops += [f"order {tag}", "rows", "seek 1 ab"]
    ops += ["close", "open ns.adt shared"]
    for tag in "abcdefghi":
        ops += [f"order {tag}", "rows", "seek 1 a"]
    ops += ["close"]
    os.makedirs(out, exist_ok=True)
    open(os.path.join(out, "setup.txt"), "w").write(SETUP)
    open(os.path.join(out, "ops.txt"), "w").write("\n".join(ops) + "\n")


if __name__ == "__main__":
    main(sys.argv[1])
