#!/usr/bin/env python3
"""Writes fixtures/write/nchar_expr/{setup,ops}.txt: orders on expressions of
NChar and NVarChar fields (UPPER, LOWER, SUBSTR, the trims, LEFT, RIGHT, the
pads, `+` with NChar, NVarChar and Char operands, IIF, COALESCE, LEN), made
before and after the records, kept current through appends, changes and
deletes, rebuilt by a reindex and a pack, and sought and scoped, for
tools/ace/write/run.sh.

    tools/ace/write/nchar_expr.py fixtures/write/nchar_expr
"""
import os
import sys

SETUP = """\
# An ADT table with NChar, NVarChar and Char fields, and one with text of
# every script (ops create the orders).
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
sql INSERT INTO ne VALUES (10, '', '', '', '')
sql CREATE TABLE ns (id INTEGER, nc NCHAR(8), nv NVARCHAR(10), c CHAR(8))
"""

# On the rows the table starts with.
FIRST = [
    "UPPER(nc)", "LOWER(nv)", "SUBSTR(nc,2,3)", "SUBSTR(nc,2)", "TRIM(nv)", "LTRIM(nv)", "ALLTRIM(nc)",
    "nc+nv", "nc+c", "c+nc", "LEFT(nc,3)", "RIGHT(nv,3)", "UPPER(nc)+c", "UPPER(nc)+LOWER(nv)",
    "SUBSTR(nc,1,2)+STR(id,3)", "PADL(nc,10)", "PADR(nc,10,'*')", "PADC(nc,12)", "PADL(nc,3)",
    "PADR(nc,12,'ab')", "PADL(nc,12,nv)", "REVERSE(nc)", "IIF(id>2,nc,nv)", "IIF(id>2,nv,nc)",
    "IIF(id>2,nc,'xyz')", "COALESCE(nc,nv)", "COALESCE(nv,nc)", "COALESCE(nc,'zz')", "COALESCE(nc,nc2,nv)",
]
# After the rows are in.
LATER = [
    "nc+SPACE(2)", "SPACE(3)+nc", "'ab'+nc", "nc+'é'", "STR(id,3)+nc", "STRZERO(id,4)+nv", "nc+c+nv",
    "c+nc+c", "LEFT(nc,20)", "RIGHT(nc,20)", "SUBSTR(nc,2,20)", "SUBSTR(nc,-3)", "SUBSTR(nc,0,2)",
    "SUBSTR(nc,3,0)", "LEFT(nc,0)", "LEFT(nc,-1)", "RIGHT(nc,0)", "RIGHT(nc,-1)", "PADL(nc,0)",
    "LEN(nc)", "LENGTH(nc)", "LEN(nc)+LEN(nv)", "LENGTH(nv)", "UPPER(c)", "UPPER(nc2)+UPPER(nv)",
    "UPPER(nc)+STR(LEN(nc),3)", "LOWER(UPPER(nc))", "TRIM(UPPER(nv))+nc2", "PADL(nc,12,'xyz')",
    "PADR(nc,12,'xyz')", "PADC(nc,13,'xyz')", "PADC(nc,12,nv)", "PADL(nc,12,'')",
]
# Their keys are numbers: sought as such.
NUMBERS = {"LEN(nc)", "LENGTH(nc)", "LEN(nc)+LEN(nv)", "LENGTH(nv)"}
# Short names for the order tags.
SHORT, LONG, XLONG = 0x20000000, 0x40000000, 0x60000000
OPTIONS = [
    ("up_s", "UPPER(nc)", SHORT), ("up_l", "UPPER(nc)", LONG), ("up_x", "UPPER(nc)", XLONG),
    ("cat_x", "nc+nv", XLONG), ("up_d", "UPPER(nc)", 8), ("uniq", "COALESCE(nc,nc2,'zz')+STR(id,3)", 1), ("up_dx", "LOWER(nv)", 8 | XLONG),
    ("sub_s", "SUBSTR(nv,2,4)", SHORT), ("c_s", "UPPER(c)", SHORT),
]

TEXTS = [
    "Привет", "привет", "Ωmega", "ωMEGA", "ÄÖÜ", "äöü", "日本語", "한국어", "ไทย", "ǅǆǄ", "ﬁnal", "straße",
    "ΑΣ", "ις", "Ǳǲǳ", "İi", "ıI", "ὈΔΥΣΣΕΎΣ", "ᾳᾼ", "ⅰⅠ", "ａｂｃ", "ⓐⒶ", "Kelvin", "€uro", "  ab",
]
ASTRAL = ["🙂a", "a🙂b", "𐐨𐐀", "𠀀x", "x𠀀"]


def wide(text):
    return text.encode("utf-16-le").hex()


def fit(text, units):
    """The start of `text` that takes no more than `units` UTF-16 units."""
    out = ""
    for c in text:
        if len((out + c).encode("utf-16-le")) > 2 * units:
            break
        out += c
    return out


def main(out):
    ops = ["# Orders on expressions of NChar and NVarChar fields.", "open ne.adt exclusive"]
    for i, expr in enumerate(FIRST):
        ops.append(f"createindex - o{i} 0 0 {expr}")
    ops += ["# Records written through them.", "append"]
    row = ["set id long 11", "set nc stringu " + wide("Привет"), "set nv stringu " + wide("Ωmega  "),
           "set c string Ab", "set nc2 stringu " + wide("é"), "write"]
    ops += row
    texts = TEXTS + ASTRAL
    for n, text in enumerate(texts):
        ops += ["append", f"set id long {12 + n}", "set nc stringu " + wide(text),
                "set nv stringu " + wide(fit(texts[(n + 5) % len(texts)], 10)),
                "set c string " + ["ab", "Ab", " x", "Äb", "zz"][n % 5],
                "set nc2 stringu " + wide(fit(texts[(n + 3) % len(texts)], 3)), "write"]
    ops += ["append", "set id long 99", "set nc null", "set nv null", "set c null", "write",
            "# Changes and deletes.", "goto 1", "set nc stringu " + wide("ЯЖЁ"), "write",
            "goto 4", "set nv stringu " + wide("zebra"), "write", "goto 6", "delete", "write",
            "goto 10", "set nc null", "write", "goto 13", "delete", "write"]
    for i, expr in enumerate(LATER):
        ops.append(f"createindex - p{i} 0 0 {expr}")
    for i, (tag, expr, options) in enumerate(OPTIONS):
        ops.append(f"createindex - {tag} {options} 0 {expr}")
    # Orders on the fields themselves, to see how a scope reads a Unicode key.
    for tag, expr in [("nc_f", "nc"), ("nv_f", "nv"), ("ncnv_f", "nc;nv")]:
        ops.append(f"createindex - {tag} 0 0 {expr}")
    ops += ["createindex ne2.adi up_for 0 512 UPPER(nc) FOR id > 4",
            "createindex ne2.adi cat_for 0 512 nc+nv FOR id < 8",
            "# More records, with the orders there.",
            "append", "set id long 70", "set nc stringu " + wide("Ŝŭŝ"), "set nv stringu " + wide("ĥŭ"),
            "set c string Ab", "write", "append", "set id long 71", "set nc stringu " + wide("        "),
            "set nv stringu " + wide("  "), "set c string x", "write",
            "close", "open ne.adt exclusive", "reindex", "pack", "close"]
    # The second table, whose text has supplementary characters (which the
    # expressions with a cut in them could split).
    ops += ["open ns.adt exclusive"]
    for tag, expr in [("a", "UPPER(nc)"), ("b", "nc+nv"), ("c", "PADL(nc,12)"), ("d", "nc+c"),
                      ("e", "LOWER(nv)"), ("f", "COALESCE(nc,nv)"), ("g", "TRIM(nv)+nc"),
                      ("h", "IIF(id>2,nc,nv)")]:
        ops.append(f"createindex - {tag} 0 0 {expr}")
    for n, text in enumerate(ASTRAL + ["a", "b😀"]):
        ops += ["append", f"set id long {n + 1}", "set nc stringu " + wide(text),
                "set nv stringu " + wide(ASTRAL[(n + 2) % len(ASTRAL)]), "set c string Zz", "write"]
    ops += ["close", "open ns.adt exclusive", "reindex", "close"]
    # Seeks and scopes.
    ops += ["open ne.adt shared", "openindex ne2.adi"]
    seeks = {"UPPER(nc)": ["ABC", "ÉTÉ", "abc", "ABCD", "A", "", "z"],
             "LOWER(nv)": ["zebra", "apple", "ZEBRA", "straße", "x"],
             "nc+nv": ["abc", "abc     Zebra", "été     apple"],
             "SUBSTR(nc,2,3)": ["bc", "bcd", "té"],
             "COALESCE(nc,nv)": ["abc", "Zebra", "apple"],
             "LEN(nc)": ["8"]}
    tags = {expr: f"o{i}" for i, expr in enumerate(FIRST)}
    tags.update({expr: f"p{i}" for i, expr in enumerate(LATER)})
    tags.update({"UPPER(nc)": "o0"})
    for expr, tag in tags.items():
        ops += [f"order {tag}", "rows"]
        if expr in NUMBERS:
            continue
        for text in seeks.get(expr, ["a", "É", "ab"]):
            for kind in (1, 2, 4):
                ops.append(f"seek {kind} {text}")
        if expr in seeks:
            ops += [f"scope 1 {seeks[expr][0]}", f"scope 2 {seeks[expr][-1]}z", "rows",
                    "getscope 1", "getscope 2", "clearscope 1", "clearscope 2"]
    for tag, _, _ in OPTIONS:
        ops += [f"order {tag}", "rows", "seek 1 ab", "seek 2 É"]
    for tag in ("nc_f", "nv_f", "ncnv_f"):
        ops += [f"order {tag}", "rows", "seek 1 abc", "seek 1 ét", "scope 1 abc", "scope 2 ég", "rows",
                "getscope 1", "getscope 2", "clearscope 1", "clearscope 2"]
    ops += ["order up_for", "rows", "order cat_for", "rows", "close"]
    ops += ["open ns.adt shared", "order a", "rows", "seek 1 a", "order b", "rows", "order c", "rows",
            "order d", "rows", "order e", "rows", "order f", "rows", "order g", "rows", "order h", "rows",
            "close"]
    os.makedirs(out, exist_ok=True)
    open(os.path.join(out, "setup.txt"), "w").write(SETUP)
    open(os.path.join(out, "ops.txt"), "w").write("\n".join(ops) + "\n")


if __name__ == "__main__":
    main(sys.argv[1])
