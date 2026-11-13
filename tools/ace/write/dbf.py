#!/usr/bin/env python3
"""Writes the DBF write scenarios for tools/ace/write/run.sh: Clipper (NTX),
FoxPro (CDX) and Visual FoxPro (VFP) tables changed through the ACE API.

    tools/ace/write/dbf.py records fixtures/write/dbf_records
    tools/ace/write/dbf.py setters fixtures/write/dbf_setters
    tools/ace/write/dbf.py memos fixtures/write/dbf_memos
    tools/ace/write/dbf.py oem fixtures/write/dbf_oem

records: every field type of each kind through the typed setters, appends,
changes written by moving away, deletes, recalls, a cancelled append,
record and table locks between two handles, read-only and exclusive opens,
VFP AutoInc, and an OEM table.
setters: every setter on every field type of each kind, as the ADT setters
scenario does, with the return code, AdsGetField's text and the field's
bytes after each.
memos: memo, binary and NMemo values: DBT sizes around its 512-byte blocks,
FPT block sizes 64, 512, 33 and 1000, values that grow and shrink (the FPT
free list, past its 82 entries), ADS VarChar overflows, NULL and empty
values, and every setter on memo fields.
oem: every ANSI byte through AdsSetString and AdsSetStringW on a Character and
a Memo field of OEM Clipper and FoxPro tables, as the client's OEM code page
(437 under Wine) takes them.
"""
import os
import random
import sys

KINDS = ("ntx", "cdx", "vfp")

# The values every setter is given, as in fixtures/write/setters.
VALUES = """string abc
string ""
string abcdefghijklmnopqrstuvwxyz
string   12.5
string T
string 11/08/2017
string {01234567-89AB-CDEF-0123-456789ABCDEF}
string \\xe4\\x80
stringw caf\\xe9
stringw abcdefghijkl
field abc
field ""
field  42
field -1.005
field 123456789.125
field 1e3
field x1
field T
field n
field 11/08/2017
field 2/30/2017
field 13:45
field 12/31/1999 23:59:59.999
field {01234567-89AB-CDEF-0123-456789ABCDEF}
field \\xde\\xad\\xbe\\xef\\x01
long 0
long -5
long 40000
long 2147483647
long -2147483648
short -32768
short 7
longlong 9223372036854775807
longlong -3
double 2.5
double -0.125
double 12345.678
double 1e20
double 0.005
logical 0
logical 1
logical 2
date 11/08/2017
date 13/45/2017
date ""
date 1/2/3
julian 2458066
julian 0
time 13:45:30.250
time 25:00
time ""
ms 0
ms 86399999
ms -1
timestamp 11/08/2017 13:45:30.250
timestamp 11/08/2017
money 123456
money -1
binary 0102
binary 0102030405060708090a
null
empty
string kept""".split("\n")


def columns(kind):
    """The fields outside the memo file that SQL creates in a table of the kind."""
    n = " NULL" if kind == "vfp" else ""
    cols = ["c CHAR(10)", "n NUMERIC(10, 2)", "n0 NUMERIC(5, 0)", "d DATE", "l LOGICAL", "b DOUBLE",
            "i INTEGER", "lg LONGINT", "g GUID", "v VARCHAR(8)", "nc NCHAR(3)", "nv NVARCHAR(4)",
            "wide CHAR(300)"]
    cols = [c + n for c in cols]
    if kind == "vfp":
        cols += ["q VARBINARY(6) NULL", "y MONEY NULL", "ts TIMESTAMP NULL", "ai AUTOINC", "v2 VARCHAR(3)",
                 "c2 CHAR(4) NOCPTRANS", "b2 DOUBLE(2)", "i2 INTEGER"]
    else:
        cols += ["b2 DOUBLE(2)"]
    return cols


def names(kind):
    return [c.split()[0] for c in columns(kind)]


def records(out):
    setup = ["# Tables of each DBF kind with every field type outside the memo file."]
    ops = []
    for kind in KINDS:
        setup += ["tabletype %s" % kind,
                  "sql CREATE TABLE r_%s (%s, m MEMO%s)" % (kind, ", ".join(columns(kind)),
                                                          " NULL" if kind == "vfp" else ""),
                  "sql CREATE TABLE p_%s (id INTEGER, c CHAR(6))" % kind]
    setup += ["# An OEM table.", "tabletype cdx", "chartype oem", "sql CREATE TABLE o_cdx (c CHAR(8), m MEMO)",
              "tabletype ntx", "sql CREATE TABLE o_ntx (c CHAR(8), m MEMO)", "chartype ansi"]
    for kind in KINDS:
        vfp = kind == "vfp"
        ops += ["# %s" % kind, "tabletype %s" % kind, "open r_%s.dbf shared" % kind, "state",
                "# A record with a value in every field.", "append", "state", "islocked 1",
                "set c string abc", "set n double 12345.67", "set n0 long 42", "set d date 11/08/2017",
                "set l logical 1", "set b double 3.5", "set i long -7", "set lg longlong 1234567890123",
                "set g string {01234567-89AB-CDEF-0123-456789ABCDEF}", "set v string var",
                "set nc stringw \\xe4\\xe9", "set nv stringw caf\\xe9", "set wide string \\@00290w",
                "set m string a memo", "set b2 double 1.25"]
        ops += (["set q binary 0102", "set y money 123456", "set ts timestamp 11/08/2017 13:45:30.250",
                 "set v2 string yy", "set c2 string raw", "set i2 long 9"] if vfp else [])
        ops += ["record", "get c", "get n", "get d", "write", "state", "islocked 1"]
        ops += ["# Strings through AdsSetField, and boundary values.", "append", "set c field zzzzzzzzzzzz",
                "set n field -999999.99", "set n0 field 99999", "set i field -2147483647",
                "set d field 01/01/0001", "set l field F", "set b field -12345.5", "write", "state"]
        ops += ["# A record with nothing set.", "append", "write", "record"]
        for f in names(kind) + ["m"]:
            ops.append("get %s" % f)
        ops += ["# NULL in every field.", "append"] + ["set %s null" % f for f in names(kind) + ["m"]]
        ops += ["write", "record"] + ["get %s" % f for f in names(kind) + ["m"]]
        ops += ["# Empty values.", "goto 1"] + ["set %s empty" % f for f in names(kind) + ["m"]]
        ops += ["record", "write", "record"]
        ops += ["# A change written by moving off the record.", "goto 2", "set c string moved", "skip 1",
                "goto 2", "get c", "state"]
        ops += ["# A change cancelled.", "goto 2", "set c string cancelled", "cancel", "get c"]
        ops += ["# An append cancelled.", "append", "set c string gone", "state", "cancel", "state", "bottom",
                "state", "record"]
        ops += ["# Deletes and recalls.", "goto 1", "delete", "state", "write", "state", "goto 1", "state",
                "recall", "state", "goto 2", "delete", "set c string deleted", "write", "state",
                "showdeleted 0", "top", "state", "goto 2", "state", "showdeleted 1", "top", "state",
                "goto 2", "recall", "skip 1", "goto 2", "state"]
        ops += ["# More appends after the deletes: no free list.", "append", "set c string tail", "write",
                "state", "append", "set c string tail2", "skip -1", "state"]
        if vfp:
            ops += ["# AutoInc.", "goto 1", "get ai", "goto 2", "get ai", "append", "get ai", "state",
                    "set ai long 99", "write", "get ai", "state"]
        ops += ["close"]
        # The second handle's record count is left out where it depends on
        # what the original engine caches per handle (see KNOWN_ISSUES.md).
        # Tables are numbered from 1 in the order they were opened; the
        # shared open after the exclusive one of each kind before fails.
        first = sum(1 for op in ops if op.startswith("open ")) - KINDS.index(kind) + 1
        a, b, c = first, first + 1, first + 3
        ops += ["# Two handles: locks.", "open p_%s.dbf shared" % kind, "open p_%s.dbf shared" % kind,
                "use %d" % a, "append", "set id long 1", "set c string one", "write", "append", "set id long 2",
                "write", "use %d" % b, "lock 1", "islocked 1", "use %d" % a, "lock 1", "goto 1",
                "set c string x", "write", "use %d" % b, "unlock 1", "use %d" % a, "lock 1", "locks", "goto 1",
                "set c string x", "write", "unlock 1", "use %d" % b, "goto 1", "set c string y", "islocked 1",
                "use %d" % a, "islocked 1", "locktable", "use %d" % b, "write", "unlock 1", "locktable",
                "use %d" % a, "tablelocked", "unlocktable", "use %d" % b, "locktable", "tablelocked", "append",
                "set id long 3", "write", "state", "unlocktable", "use %d" % a, "bottom", "state", "append",
                "set id long 4", "use %d" % b, "append", "set id long 5", "write", "use %d" % a, "write",
                "use %d" % b, "state", "close", "use %d" % a, "close", "open p_%s.dbf readonly" % kind, "goto 1",
                "set c string ro", "append", "delete", "close", "open p_%s.dbf exclusive" % kind,
                "open p_%s.dbf shared" % kind, "use %d" % c, "append", "set id long 6", "write", "state",
                "close"]
    ops += ["# The OEM tables.", "chartype oem"]
    for kind in ("cdx", "ntx"):
        ops += ["tabletype %s" % kind, "open o_%s.dbf shared" % kind, "append", "set c string \\xe4\\xf6\\xfc\\xdf",
                "set m string \\xc4\\xd6\\xdc memo", "write", "raw c", "get c", "append",
                "set c string \\x80\\x9c", "get c", "raw c", "set c stringw \\x80", "set c string ok",
                "write", "raw c", "set m string \\x80", "write", "close"]
    ops += ["chartype ansi"]
    write(out, setup, ops)


def setters(out):
    setup = ["# One field of every type of each kind, for the setters."]
    ops = []
    for kind in KINDS:
        setup += ["tabletype %s" % kind, "sql CREATE TABLE s_%s (%s)" % (kind, ", ".join(columns(kind)))]
        ops += ["tabletype %s" % kind, "open s_%s.dbf" % kind, "append"]
        for f in names(kind):
            ops.append("# %s %s" % (kind, f))
            for value in VALUES:
                ops += ["set %s %s" % (f, value), "get %s" % f, "raw %s" % f]
        ops += ["write", "state", "close"]
    write(out, setup, ops)


def text(size, mark):
    """`size` bytes of text, as the \\@ escape repeats a character."""
    if size == 0:
        return '""'
    return "%s\\@%05d%s" % (mark, size - 1, mark) if size > 1 else mark


def binary(size, seed):
    rng = random.Random(seed)
    return "".join("%02x" % rng.randrange(256) for _ in range(size)) or '""'


FPT_SIZES = [0, 1, 2, 55, 56, 57, 63, 64, 65, 119, 120, 121, 184, 300, 1000, 4000]
DBT_SIZES = [0, 1, 2, 300, 510, 511, 512, 513, 1022, 1023, 1024, 1025, 1535, 1536, 3000, 6000]
MEMO_SETTERS = ["string abc", 'string ""', "string \\@03000x", "stringw caf\\xe9", "field abc", 'field ""',
                "binary 0102", "binary 00", 'binary ""', "image 89504e47", "long 5", "double 2.5",
                "date 11/08/2017", "null", "empty", "string kept"]


def memos(out):
    setup = ["# Memo fields of each kind, FPT block sizes and ADS VarChar overflows."]
    tables = []
    for kind in KINDS:
        n = " NULL" if kind == "vfp" else ""
        sizes = [""] if kind == "ntx" else ["", " MEMOBLOCKSIZE 1", " MEMOBLOCKSIZE 33", " MEMOBLOCKSIZE 1000"]
        setup.append("tabletype %s" % kind)
        for i, size in enumerate(sizes):
            name = "m%d_%s" % (i, kind)
            setup.append("sql CREATE TABLE %s (id INTEGER, m MEMO%s, b BLOB%s, nm NMEMO%s)%s"
                         % (name, n, n, n, size))
            tables.append((kind, name))
        if kind != "vfp":
            setup.append("sql CREATE TABLE vc_%s (id INTEGER, v VARCHAR(8), w VARCHAR(70))" % kind)
            setup.append("sql CREATE TABLE api_%s (id INTEGER, m MEMO, b BLOB)" % kind)
        else:
            setup.append("sql CREATE TABLE vv_vfp (id INTEGER, v VARCHAR(6) NULL, q VARBINARY(6) NULL, "
                         "w VARCHAR(6))")
        setup.append("sql CREATE TABLE fl_%s (id INTEGER, m MEMO)" % kind)
    rng = random.Random(11)
    ops = []
    for kind, name in tables:
        sizes = DBT_SIZES if kind == "ntx" else FPT_SIZES
        ops += ["# %s" % name, "tabletype %s" % kind, "open %s.dbf shared" % name]
        live, records = [], 0
        marks = iter("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ" * 40)
        for step in range(160):
            x = rng.random()
            if x < 0.4 or not live:
                records += 1
                ops += ["append", "set id long %d" % records]
                for f in rng.sample("mbn", 3):
                    size = rng.choice(sizes)
                    if f == "m":
                        ops.append("set m string %s" % text(size, next(marks)))
                    elif f == "b":
                        ops.append("set b binary %s" % binary(min(size, 3000), step))
                    else:
                        ops.append("set nm stringw %s" % text(size // 2, next(marks)))
                ops += ["write"]
                live.append(records)
            elif x < 0.9:
                record = rng.choice(live)
                ops.append("goto %d" % record)
                for f in rng.sample("mbn", rng.randrange(1, 4)):
                    field = {"m": "m", "b": "b", "n": "nm"}[f]
                    if rng.random() < 0.08:
                        ops.append("set %s null" % field)
                        continue
                    size = rng.choice(sizes)
                    if f == "m":
                        ops.append("set m string %s" % text(size, next(marks)))
                    elif f == "b":
                        ops.append("set b binary %s" % binary(min(size, 3000), step))
                    else:
                        ops.append("set nm stringw %s" % text(size // 2, next(marks)))
                ops += ["write", "get m"]
            else:
                record = live.pop(rng.randrange(len(live)))
                ops += ["goto %d" % record, "delete", "write"]
        ops += ["# A change cancelled, a delete with a change pending, a value set twice.",
                "top", "set m string %s" % text(70, "Q"), "set b binary %s" % binary(40, 99), "cancel", "get m",
                "top", "set m string %s" % text(30, "R"), "set m string %s" % text(700, "S"), "write",
                "skip 1", "set m string %s" % text(20, "T"), "delete", "write", "close"]
    for kind in KINDS:
        ops += ["# %s: the free list" % kind, "tabletype %s" % kind, "open fl_%s.dbf shared" % kind]
        for i in range(1, 121):
            ops += ["append", "set id long %d" % i, "set m string s%d" % i, "write"]
        for i in range(1, 121, 1 if kind == "vfp" else 2):
            ops += ["goto %d" % i, "set m string %s" % text(200, "g"), "write"]
        for i in range(1, 40):
            ops += ["append", "set m string %s" % text(i % 3 * 60 + 1, "r"), "write"]
        ops += ["close"]
        if kind == "vfp":
            ops += ["# VFP VarChar and VarBinary", "open vv_vfp.dbf shared"]
            for i in range(120):
                if i % 3 == 0 or i < 4:
                    ops.append("append")
                else:
                    ops.append("goto %d" % rng.randrange(1, i // 3 + 2))
                for f in rng.sample("vqw", rng.randrange(1, 4)):
                    if f != "w" and rng.random() < 0.15:
                        ops.append("set %s null" % f)
                        continue
                    size = rng.randrange(0, 8)
                    if f == "q":
                        ops.append("set q binary %s" % binary(size, i))
                    else:
                        ops.append("set %s string %s" % (f, text(size, "abc"[i % 3])))
                ops += ["write", "raw v", "raw q", "get v", "get q"]
            ops += ["close"]
            continue
        ops += ["# %s: ADS VarChar overflows" % kind, "open vc_%s.dbf shared" % kind]
        live, records = [], 0
        for i in range(250):
            def value():
                if rng.random() < 0.08:
                    return "null"
                size = rng.choice((0, 1, 4, 5, 6, 7, 8, 9, 20, 56, 57, 63, 64, 65, 70, 100, 520))
                return "string %s" % text(size, "abcdef"[i % 6])
            if rng.random() < 0.4 or not live:
                records += 1
                ops += ["append", "set id long %d" % records, "set v %s" % value(), "set w %s" % value(), "write"]
                live.append(records)
            else:
                ops += ["goto %d" % rng.choice(live), "set %s %s" % (rng.choice("vw"), value()), "write",
                        "get v", "get w"]
        ops += ["close", "# %s: every setter on memo fields" % kind, "open api_%s.dbf shared" % kind, "append"]
        for f in ("m", "b"):
            for value in MEMO_SETTERS:
                ops += ["set %s %s" % (f, value), "get %s" % f, "raw %s" % f]
            ops += ["write"]
        ops += ["append", "set b image ffd8ff", "set m string \\@00600x", "write", "goto 1",
                "set b binary 00", "write", "close"]
    for kind in ("cdx", "vfp"):
        ops += ["# %s: every setter on memo fields" % kind, "tabletype %s" % kind, "open m0_%s.dbf shared" % kind,
                "append"]
        for f in ("m", "b", "nm"):
            for value in MEMO_SETTERS:
                ops += ["set %s %s" % (f, value), "get %s" % f, "raw %s" % f]
            ops += ["write"]
        ops += ["close"]
    write(out, setup, ops)


def oem(out):
    setup = ["# OEM tables: the client's text converted to its OEM code page.", "chartype oem"]
    ops = ["chartype oem"]
    for kind in ("ntx", "cdx"):
        setup += ["tabletype %s" % kind, "sql CREATE TABLE oem_%s (c CHAR(1), m MEMO)" % kind]
        ops += ["tabletype %s" % kind, "open oem_%s.dbf" % kind, "append"]
        for byte in range(1, 256):
            ops += ["set c string \\x%02x" % byte, "raw c", "set c stringw \\x%02x" % byte, "raw c"]
        for byte in range(0x80, 0x100, 8):
            text = "".join("\\x%02x" % b for b in range(byte, byte + 8))
            ops += ["set m string %s" % text, "write", "get m", "set m stringw %s" % text, "write", "get m"]
        ops += ["close"]
    write(out, setup, ops)


def write(out, setup, ops):
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "setup.txt"), "w") as f:
        f.write("\n".join(setup) + "\n")
    with open(os.path.join(out, "ops.txt"), "w") as f:
        f.write("\n".join(ops) + "\n")


if __name__ == "__main__":
    {"records": records, "setters": setters, "memos": memos, "oem": oem}[sys.argv[1]](sys.argv[2])
