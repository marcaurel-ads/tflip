#!/usr/bin/env python3
"""Writes fixtures/write/limits/{setup,ops}.txt: the limits the original
engine enforces on tables and orders made through the ACE API, on each table
type (field lengths, memo block sizes, names, columns, tags and open index
files in an order, key, expression and condition lengths), for
tools/ace/write/run.sh.

    tools/ace/write/limits.py fixtures/write/limits

Names stay short: a path's limit counts the data folder too, which differs
between where the original and a test run.
"""
import os
import sys

TYPES = [("adt", "adt", "a"), ("ntx", "dbf", "n"), ("cdx", "dbf", "c"), ("vfp", "dbf", "v")]

out = []


def op(line):
    out.append(line)


def create_table(line, info=False):
    """A table is closed at once, so that few stay open. A failed create
    leaves the last table the current one, so its close fails too."""
    op(line)
    if info:
        op("info")
    op("close")


for table_type, ext, p in TYPES:
    count = [0]

    def name():
        count[0] += 1
        return "%s%d.%s" % (p, count[0], ext)

    op("tabletype " + table_type)
    char = "VarCharFox" if table_type == "vfp" else "VarChar"
    # Field lengths and decimals.
    for definition in [
        "NChar,32500", "NChar,32501", "NVarChar,32500", "NVarChar,32501", "NChar,0", "NVarChar,0",
        "Character,254", "Character,255", "Character,256", "Character,1024", "Character,65000", "Character,65530",
        "Character,65531", "Character,65535", "Character,0", "Character,1",
        char + ",254", char + ",255", char + ",256", char + ",1000", char + ",5", char + ",6", char + ",65000",
        char + ",65001", char + ",0", "CICharacter,65000", "CICharacter,65001", "Raw,65000", "Raw,65001", "VarBinaryFox,254", "VarBinaryFox,255", "VarBinary,255",
        "VarBinary,65000", "VarBinaryFox,65000", "VarBinaryFox,65001", "Binary,0",
        "Numeric,0", "Numeric,1", "Numeric,2", "Numeric,19", "Numeric,20", "Numeric,32", "Numeric,33", "Numeric,64",
        "Numeric,255", "Numeric,256", "Numeric,32,30", "Numeric,32,31", "Numeric,32,32", "Numeric,20,18",
        "Numeric,20,19", "Numeric,3,1", "Numeric,2,1", "Numeric,10,255", "Double,8,32", "Double,8,31", "Double,8,15",
        "Double,8,33", "Double,8,255", "Double,8,256", "CurDouble,8,2", "Short", "Integer,4", "Integer,4,2",
    ]:
        create_table("createtable %s 0 a,%s" % (name(), definition))
    # Memo block sizes.
    for size in [0, 1, 2, 3, 4, 5, 7, 8, 9, 16, 31, 32, 33, 64, 100, 128, 255, 256, 257, 512, 513, 1024, 2048,
                 4096, 8192, 16384, 32768, 32769, 65535]:
        create_table("createtable %s %d a,Character,5;m,Memo" % (name(), size), True)
    # Field names.
    for length in [10, 11, 20, 127, 128, 129, 130, 255, 256]:
        create_table("createtable %s 0 %s,Integer" % (name(), "f" * length))
    for length in [10, 11, 100]:
        create_table("createtable %s 0 a,Integer;%s,Integer" % (name(), "g" * length))
    # Orders.
    cols = ";".join("c%d,Character,2" % i for i in range(1, 22))
    lengths = [100, 200, 240, 241, 244, 250, 256, 257, 260, 300, 500, 512, 513, 1000, 4000, 4081, 4082, 4083, 4084,
               8000]
    longs = ";".join("l%d,Character,%d" % (n, n) for n in lengths)
    op("createtable %s 0 %s;%s" % (name(), cols, longs))
    ntx = table_type == "ntx"
    made = [0]

    def create(tag, options, page, expression, cond=""):
        # An NTX table takes 15 index files: a new table every ten orders
        # keeps that limit out of the other checks.
        if ntx and made[0] and made[0] % 10 == 0:
            op("close")
            op("createtable %s 0 %s;%s" % (name(), cols, longs))
        made[0] += 1
        file = "%s.ntx" % tag if ntx else "-"
        line = "createindex %s %s %d %d %s" % (file, tag, 0 if ntx else options, page, expression)
        op(line + (" FOR " + cond if cond else ""))

    joiner = ";" if table_type == "adt" else "+"
    for k in [14, 15, 16, 17, 21]:
        create("k%d" % k, 2, 512, joiner.join("c%d" % i for i in range(1, k + 1)))
    if table_type == "adt":
        for k in [15, 16]:
            create("p%d" % k, 2, 512, "+".join("c%d" % i for i in range(1, k + 1)))
    for n in lengths:
        create("l%d" % n, 2, 512, "l%d" % n)
    # The page size of an ADI's order with a long key.
    if table_type == "adt":
        for n, page in [(300, 512), (1000, 1024), (2000, 2048), (4000, 4096), (4082, 8192), (1000, 512), (2000, 512),
                        (4082, 512), (200, 8192), (200, 1024), (200, 16384), (200, 100), (200, 513)]:
            create("s%d_%d" % (n, page), 2, page, "l%d" % n)
    # The page size a new ADI file takes by default (0) for one long key.
    if table_type == "adt":
        op("close")
        for n in [100, 158, 159, 200, 240, 243, 330, 498, 1000, 2034, 4082]:
            op("createtable %s 0 k,Character,%d" % (name(), n))
            op("createindex - t 2 0 k")
            op("close")
        op("createtable %s 0 %s;%s" % (name(), cols, longs))
    # Tag names.
    for length in [10, 11, 12, 127, 128, 129, 130]:
        create("t" * length, 2, 512, "c1")
    # Expressions and conditions of many characters.
    for length in [100, 200, 250, 255, 256, 257, 260, 300, 400, 500, 508, 509, 510, 511, 512, 513, 520, 600]:
        create("x%d" % length, 2, 512, "c1+" + " " * (length - 5) + "''")
        create("y%d" % length, 2, 512, "c1", "c1 <> '%s'" % ("x" * (length - 9)))
    # Many orders on one table.
    op("close")
    many = name()
    op("createtable %s 0 %s" % (many, cols))
    if ntx:
        for i in range(1, 19):
            create("o%d" % i, 0, 512, "c%d" % (i % 21 + 1))
    else:
        # The original takes 256 tags per index file; fixtures/write/tagdir
        # has all of them, and the 257th.
        for i in range(1, 61):
            create("o%d" % i, 2, 512, "c%d" % (i % 21 + 1))
    op("close")
    if ntx:
        # 15 index files are open on a table at most, however they get open.
        op("open " + many)
        for i in range(1, 4):
            op("createindex p%d.ntx p%d 0 512 c%d" % (i, i, i + 1))
        op("close")
        op("open " + many)
        for i in range(1, 16):
            op("openindex o%d.ntx" % i)
        op("openindex p1.ntx")
        op("openindex p2.ntx")
        op("close")
    # The number of fields.
    for count_ in [255, 256, 257, 258]:
        create_table("createtable %s 0 %s" % (name(), ";".join("f%d,Short" % i for i in range(1, count_ + 1))))
    # A record's length.
    for length in [32, 33]:
        create_table("createtable %s 0 %s" % (name(), ";".join("f%d,Character,1000" % i for i in range(1, length + 1))))

    # The same on a table opened again, with its orders open.
    op("open " + many)
    op("info")
    op("close")

os.makedirs(sys.argv[1], exist_ok=True)
with open(os.path.join(sys.argv[1], "setup.txt"), "w") as f:
    # The before/ folder needs a file to be kept in the repository.
    f.write("createtable base.adt 0 a,Integer\nclose\n")
with open(os.path.join(sys.argv[1], "ops.txt"), "w") as f:
    f.write("# The limits of tables and orders made through the API, on each table type\n")
    f.write("# (generated by tools/ace/write/limits.py).\n")
    f.write("\n".join(out) + "\n")
