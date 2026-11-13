#!/usr/bin/env python3
"""Writes fixtures/write/{bitmap_split,bitmap_levels,cdx_bitmap_split,
cdx_bitmap_levels}/{setup,ops}.txt: binary orders whose bitmaps outgrow the 62
leaves of their root node (512-byte nodes: 3,968 records a leaf), for
tools/ace/write/run.sh.

The tables have one CHAR(1) column, to keep their files small, and are made
by cross joins of the numerals 0 to 9. An append reuses a deleted record, so
an ADT table has none until the script deletes some.

    tools/ace/write/bitmap_levels.py fixtures/write
"""
import os
import sys

EXPR = "a.n * 100000 + b.n * 10000 + c.n * 1000 + e.n * 100 + f.n * 10 + g.n"
D = ["sql CREATE TABLE d (n INTEGER)", "sql INSERT INTO d VALUES (0), (1), (2), (3), (4), (5), (6), (7), (8), (9)"]


def table(name, n, delete=True):
    out = [
        f"sql CREATE TABLE {name} (x CHAR(1))",
        f"sql INSERT INTO {name} SELECT CASE WHEN ({EXPR}) % 7 = 0 THEN 'b' ELSE 'a' END FROM d a, d b, d c, d e, d f, d g WHERE {EXPR} < {n}",
    ]
    if delete:
        out.append(f"sql DELETE FROM {name} WHERE x = 'b'")
    return out


def nav(skip):
    return ["obottom", "pos", "oskip -1", "pos", "otop", "pos", f"oskip {skip}", "pos", "oskip 1", "pos"]


def put(out, name, setup, ops, header):
    os.makedirs(f"{out}/{name}", exist_ok=True)
    open(f"{out}/{name}/setup.txt", "w").write("\n".join(header + setup) + "\n")
    open(f"{out}/{name}/ops.txt", "w").write("\n".join(ops) + "\n")


def append(*values):
    out = []
    for v in values:
        out += ["append", f"set x string {v}", "write"]
    return out


def flip(*ops):
    out = []
    for kind, r in ops:
        out += [f"goto {r}", kind, "write"]
    return out


def change(*ops):
    out = []
    for r, v in ops:
        out += [f"goto {r}", f"set x string {v}", "write"]
    return out


def adt_idx(t, tag):
    return f"sql EXECUTE PROCEDURE sp_CreateIndex90 ('{t}', '{t}', '{tag}', 'DELETED()', NULL, 4098, 512, NULL)"


def cdx_idx(*tags):
    out = []
    for tag, expr in tags:
        out.append(f"createindex - {tag} 4098 512 {expr}")
    return out


def main(out):
    # ADT, the first split of the root's 62 leaves
    ops = [
        "# A binary order whose bitmap fills the 62 leaves of its root, then starts a",
        "# 63rd: the leaves move under an interior node, and a second one holds the",
        "# new leaf. Records deleted in both (an append reuses a deleted record, so none",
        "# before), then rebuilt. Snapshots of closed files: the index file in them is",
        "# compared too.",
        adt_idx('a', 'del'),
        "open a.adt exclusive", "order del", "count", *nav(210000),
        *append('b', 'a'),
        *flip(("delete", 5), ("delete", 245000), ("delete", 246016)),
        "order del", "count", *nav(211000),
        "close",
        "snapshot split",
        "open a.adt exclusive",
        "reindex",
        "order del", "count", *nav(211000),
        "close",
    ]
    put(out, 'bitmap_split', table('a', 246015, False), ops, D)

    # ADT, a root over two full interior nodes
    ops = [
        "# A binary order whose bitmap takes 124 leaves, under two full interior nodes:",
        "# a 125th leaf needs a third. Records deleted in each node, a",
        "# second order built after it, then both rebuilt. Snapshots of closed files:",
        "# the index files in them are compared too.",
        adt_idx('c', 'del'),
        adt_idx('c', 'del2'),
        "open c.adt exclusive", "order del", "count", *nav(400000),
        "order del2", "count", "obottom", "pos", "close",
        "open c.adt exclusive",
        *append('b', 'a', 'b'),
        *flip(("delete", 3), ("delete", 100000), ("delete", 246016), ("delete", 492032)),
        "order del", "count", *nav(420000),
        "order del2", "count", "obottom", "pos", "oskip -1", "pos",
        "close",
        "snapshot grown",
        "open c.adt exclusive",
        "reindex",
        "order del", "count", "obottom", "pos", "oskip -1", "pos", "otop", "pos", "oskip 400000", "pos",
        "close",
    ]
    put(out, 'bitmap_levels', table('c', 492031, False), ops, D)


    # CDX, the first split
    ops = [
        "# Binary orders whose bitmaps fill the 62 leaves of their root, then start a",
        "# 63rd, as in a binary ADI order (see bitmap_split); a changed record flips",
        "# the bit of an order on a value.",
        "tabletype cdx",
        "open a.dbf exclusive",
        *cdx_idx(("del", "DELETED()"), ("high", "x > 'a'")),
        "order del", "count", *nav(210000),
        "order high", "count", "obottom", "pos", "otop", "pos", "oskip 20000", "pos",
        *append('b', 'a'),
        *change((100, 'b'), (246015, 'b'), (246016, 'a'), (14, 'a'), (245000, 'b')),
        *flip(("delete", 5), ("delete", 245000), ("delete", 246016), ("recall", 8)),
        "order del", "count", *nav(211000),
        "order high", "count", "obottom", "pos", "otop", "pos", "oskip 20000", "pos",
        "close",
    ]
    put(out, 'cdx_bitmap_split', table('a', 246015), ops, ["tabletype cdx"] + D)

    # CDX, a partial second interior node, and a third
    ops = [
        "# Binary orders of 76 leaves (the root over a full interior node and one of 14",
        "# leaves) and of 124 (two full ones), kept current as records are added, past",
        "# the 124th leaf too, and the second rebuilt.",
        "tabletype cdx",
        "open m.dbf exclusive",
        *cdx_idx(("del", "DELETED()")),
        "order del", "count", *nav(250000),
        *append('b', 'a'),
        *flip(("delete", 249984), ("delete", 300000), ("delete", 8), ("recall", 8), ("recall", 300000), ("delete", 300002)),
        "order del", "count", *nav(260000),
        "close",
        "open c.dbf exclusive",
        *cdx_idx(("del", "DELETED()")),
        "order del", "count", "obottom", "pos", "otop", "pos", "oskip 400000", "pos",
        *append('b', 'a', 'b'),
        *flip(("delete", 3), ("delete", 100000), ("delete", 246016), ("delete", 492032), ("recall", 8), ("recall", 399995)),
        "order del", "count", *nav(420000),
        "reindex",
        "order del", "count", *nav(420000),
        "close",
    ]
    put(out, 'cdx_bitmap_levels', table('m', 300000) + table('c', 492031), ops, ["tabletype cdx"] + D)


if __name__ == "__main__":
    main(sys.argv[1])
