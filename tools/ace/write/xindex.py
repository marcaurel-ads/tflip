#!/usr/bin/env python3
"""Writes the DBF index scenarios for tools/ace/write/run.sh: records written
through the ACE API to Clipper tables with .ntx indexes, and to FoxPro and
Visual FoxPro tables with orders in a structural .cdx, a second .cdx and
single-order .idx files.

    tools/ace/write/xindex.py ntx fixtures/write/ntx_indexes
    tools/ace/write/xindex.py cdx fixtures/write/cdx_indexes

Each table gets its orders before any data; a seeded random run then
appends, changes (some fields, NULL now and then), deletes and recalls
records, so that pages split, empty and come back from the free lists. One
table per scenario grows to 3,000 records for deeper trees.
"""
import os
import random
import sys

WORDS = ["abc", "ABC", "Abc", "\\xe4pfel", "\\xc4pfel", "apfel", "zebra", "Zo\\xeb", "\\xe9lan", "\\xc9lan",
         "stra\\xdfe", "STRASSE", "m\\xfcller", "Muller", '""', "  lead", "x", "zz", "r\\xe9sum\\xe9", "\\xdc",
         "k1", "k22"]


def sp(table, name, expr, cond=None, options=2, file=None, collation=None):
    q = lambda v: "NULL" if v is None else "'%s'" % v.replace("'", "''")
    return "sql EXECUTE PROCEDURE sp_CreateIndex90 ('%s', %s, %s, %s, %s, %d, 0, %s)" % (
        table, q(file or table), q(name), q(expr), q(cond), options, q(collation))


def ntx_sp(table, name, expr, cond=None, options=0):
    q = lambda v: "NULL" if v is None else "'%s'" % v.replace("'", "''")
    return "sql EXECUTE PROCEDURE sp_CreateIndex90 ('%s', '%s', NULL, %s, %s, %d, 0, NULL)" % (
        table, name, q(expr), q(cond), options)


class Run:
    """A seeded random run of record operations on one table, by record number."""

    def __init__(self, seed, fields, keyspace=40):
        self.rng = random.Random(seed)
        self.fields = fields
        self.keyspace = keyspace
        self.records = 0
        self.live = []
        self.dead = []
        self.ops = []

    def value(self, field):
        rng = self.rng
        if rng.random() < 0.08 and field not in ("id",):
            return "null"
        kind = self.fields[field]
        if kind == "k":
            return "long %d" % rng.randrange(self.keyspace)
        if kind == "name":
            return "string %s%s" % (rng.choice(WORDS), rng.choice(["", "a", "\\xc4", "1"]))
        if kind == "l":
            return "logical %d" % (rng.random() < 0.3)
        if kind == "d":
            return "date %02d/%02d/%04d" % (rng.randrange(1, 13), rng.randrange(1, 29), rng.randrange(1900, 2100))
        if kind == "n":
            return "double %.2f" % (rng.randrange(-99999, 99999) / 100)
        if kind == "m":
            n = rng.choice([0, 1, 30, 100, 600])
            return "string %s" % ("".join(rng.choice("abcx ") for _ in range(n)) or '""')
        if kind == "bl":
            n = rng.choice([0, 1, 50, 300])
            return "binary %s" % ("".join("%02x" % rng.randrange(256) for _ in range(n)) or '""')
        if kind == "v":
            n = rng.randrange(0, 9)
            return "string %s" % ("".join(rng.choice("abc") for _ in range(n)) or '""')
        if kind == "y":
            return "money %d" % rng.randrange(-10 ** 8, 10 ** 8)
        if kind == "ts":
            return "timestamp %02d/%02d/%04d %02d:%02d:%02d.%03d" % (
                rng.randrange(1, 13), rng.randrange(1, 29), rng.randrange(1990, 2030), rng.randrange(24),
                rng.randrange(60), rng.randrange(60), rng.randrange(1000))
        if kind == "b":
            return "double %.2f" % rng.uniform(-1e6, 1e6)
        raise ValueError(kind)

    def set_fields(self, names):
        for field in names:
            self.ops.append("set %s %s" % (field, self.value(field)))

    def step(self, recall=True, dup_id=False):
        rng = self.rng
        x = rng.random()
        names = [f for f in self.fields if f != "id"]
        if x < 0.45 or not self.live:
            self.records += 1
            self.ops += ["append", "set id long %d" % self.records]
            self.set_fields(rng.sample(names, len(names)))
            self.ops.append("write")
            self.live.append(self.records)
        elif x < 0.82:
            record = rng.choice(self.live)
            self.ops.append("goto %d" % record)
            if dup_id and rng.random() < 0.1:
                self.ops += ["set id long %d" % rng.choice(self.live + self.dead), "write", "cancel"]
                return
            self.set_fields(rng.sample(names, rng.randrange(1, len(names) + 1)))
            self.ops.append("write")
        elif x < 0.92 or not self.dead or not recall:
            record = self.live.pop(rng.randrange(len(self.live)))
            self.dead.append(record)
            self.ops += ["goto %d" % record, "delete", "write"]
        else:
            record = self.dead.pop(rng.randrange(len(self.dead)))
            self.live.append(record)
            self.ops += ["goto %d" % record, "recall", "write"]


NTX_FIELDS = {"id": "id", "c": "name", "n": "n", "d": "d", "l": "l", "b": "b", "m": "m", "bl": "bl"}
NTX_TABLE = "(id INTEGER, c CHAR(10), n NUMERIC(8, 2), d DATE, l LOGICAL, b DOUBLE(2), m MEMO, bl BLOB)"


def ntx_indexes(t):
    return ["sql CREATE INDEX %s_id ON %s (id)" % (t, t), "sql CREATE INDEX %s_c ON %s (c)" % (t, t),
            "sql CREATE INDEX %s_n ON %s (n)" % (t, t), "sql CREATE INDEX %s_d ON %s (d)" % (t, t),
            "sql CREATE INDEX %s_l ON %s (l)" % (t, t), "sql CREATE INDEX %s_b ON %s (b)" % (t, t),
            ntx_sp(t, "%s_up" % t, "UPPER(c) + DTOS(d)"), ntx_sp(t, "%s_fd" % t, "c", ".NOT. DELETED()"),
            ntx_sp(t, "%s_fn" % t, "n", "n > 0"), ntx_sp(t, "%s_u" % t, "c", None, 1),
            ntx_sp(t, "%s_dc" % t, "c", None, 8), ntx_sp(t, "%s_st" % t, "STR(n) + c", "l .OR. DELETED()")]


def ntx(out):
    setup = ["# Clipper tables with .ntx indexes, created before any data.", "tabletype ntx"]
    ops = ["tabletype ntx"]
    for t in ("t", "w"):
        setup.append("sql CREATE TABLE %s %s" % (t, NTX_TABLE))
        setup += ntx_indexes(t)
    setup += ["sql CREATE TABLE big (id INTEGER, c CHAR(10), n NUMERIC(8, 2))",
              "sql CREATE INDEX big_id ON big (id)", "sql CREATE INDEX big_c ON big (c)",
              "sql CREATE INDEX big_n ON big (n)", ntx_sp("big", "big_x", "c + STR(n, 8, 2)")]

    def opens(t, names):
        return ["open %s.dbf shared" % t] + ["openindex %s.ntx" % n for n in names]

    names = lambda t: ["%s_%s" % (t, s) for s in ("id", "c", "n", "d", "l", "b", "up", "fd", "fn", "u", "dc", "st")]
    run = Run(28, NTX_FIELDS)
    for _ in range(900):
        run.step()
    ops += ["# t: a random run"] + opens("t", names("t")) + run.ops + ["close"]
    ops += ["# w: a Double too wide for its key, a key that cannot be built"] + opens("w", names("w"))
    ops += ["append", "set id long 1", "set c string one", "set b double 1.5", "write", "append",
            "set id long 2", "set c string wide", "set b double 1e12", "write", "state", "cancel", "state",
            "goto 1", "set b double 1e15", "write", "cancel", "set c string ok", "set n double 1.5", "write",
            "append", "set id long 3", "set c string three", "set m string \\@00700x", "write", "goto 3",
            "set b double -1e13", "set c string no", "write", "state", "cancel", "goto 3", "set m string y",
            "write", "delete", "write", "bottom", "state", "close"]
    big = Run(29, {"id": "id", "c": "name", "n": "n"}, keyspace=3000)
    for i in range(1, 3001):
        big.records += 1
        big.live.append(i)
        big.ops += ["append", "set id long %d" % i, "set c %s" % big.value("c"),
                    "set n double %.2f" % (big.rng.randrange(-99999, 99999) / 100), "write"]
    for _ in range(1500):
        big.step()
    for record in range(1, 3001, 3):
        big.ops += ["goto %d" % record, "set c %s" % big.value("c"), "write"]
    ops += ["# big: 3,000 records, then a random run"] + opens("big", ["big_id", "big_c", "big_n", "big_x"])
    ops += big.ops + ["close"]
    write(out, setup, ops)


CDX_TABLE = "(id INTEGER, k INTEGER, name CHAR(12), l LOGICAL, d DATE, n NUMERIC(8, 2), m MEMO, bl BLOB)"
VFP_TABLE = ("(id INTEGER, k INTEGER NULL, name CHAR(12) NULL, l LOGICAL, d DATE NULL, n NUMERIC(8, 2), "
             "m MEMO NULL, v VARCHAR(8) NULL, y MONEY)")
CDX_FIELDS = {"id": "id", "k": "k", "name": "name", "l": "l", "d": "d", "n": "n", "m": "m", "bl": "bl"}
VFP_FIELDS = {"id": "id", "k": "k", "name": "name", "l": "l", "d": "d", "n": "n", "m": "m", "v": "v", "y": "y"}


def orders(t):
    return ["sql CREATE INDEX by_k ON %s (k)" % t, "sql CREATE INDEX by_d ON %s (d)" % t,
            sp(t, "up", "UPPER(name)"), sp(t, "x", "name + DTOS(d)"), sp(t, "nd", "id", ".NOT. DELETED()"),
            sp(t, "del", "DELETED()"), sp(t, "c", "name", "k > 20"), sp(t, "u", "k", options=3),
            sp(t, "desc", "n", options=10), sp(t, "bl", "l", options=4098), sp(t, "bd", "DELETED()", options=4098),
            sp(t, "fd", "k", "DELETED()")]


def cdx(out):
    setup = ["# FoxPro tables: a structural .cdx with many orders, a second .cdx, .idx files.", "tabletype cdx",
             "sql CREATE TABLE c %s" % CDX_TABLE] + orders("c")
    setup += ["sql CREATE TABLE s %s" % CDX_TABLE, "sql CREATE INDEX by_k ON s (k)",
              sp("s", "s_name", "UPPER(name)", file="second"), sp("s", "s_del", "id", ".NOT. DELETED()", file="second"),
              sp("s", "s_u", "k", options=3, file="second")]
    idx = [("bk", "k", None, 0), ("nm", "name", None, 0), ("ex", "name + DTOS(d)", None, 0), ("dn", "n", None, 8),
           ("fc", "name", "k > 20", 0), ("uq", "k", None, 1), ("ndx", "id", ".NOT. DELETED()", 0),
           ("bx", "l", None, 4096)]
    setup += ["sql CREATE TABLE i %s" % CDX_TABLE] + [sp("i", n, e, c, o, file=n) for n, e, c, o in idx]
    setup += ["sql CREATE TABLE big (id INTEGER, k INTEGER, name CHAR(12))", "sql CREATE INDEX by_k ON big (k)",
              sp("big", "x", "name + STR(k, 5)"), sp("big", "nd", "id", ".NOT. DELETED()")]
    # No TimeStamp keys: tflip does not build them (the original rounds them
    # its own way), and tables with such orders refuse writes.
    setup += ["# Visual FoxPro tables: candidate, NULL keys, segments.",
              "tabletype vfp", "sql CREATE TABLE v %s" % VFP_TABLE, sp("v", "cand", "id", options=2050)]
    setup += orders("v") + [sp("v", "seg", "name;k"), sp("v", "vv", "v"), sp("v", "yy", "y"),
                            sp("v", "un", "name", options=3), sp("v", "cn", "UPPER(name)", "k > 5 .AND. .NOT. DELETED()")]
    setup += ["sql CREATE TABLE vi %s" % VFP_TABLE] + [sp("vi", n, e, None, o, file=n) for n, e, o in
                                                        (("vbk", "k", 0), ("vnm", "name", 0), ("vseg", "name;k", 0),
                                                         ("vdn", "n", 8), ("vuq", "k", 1))]
    ops = ["tabletype cdx"]
    for t, seed, extra in (("c", 21, []), ("s", 33, ["second.cdx"]), ("i", 73, [n + ".idx" for n, *_ in idx])):
        run = Run(seed, CDX_FIELDS)
        for _ in range(800):
            run.step()
        ops += ["# %s: a random run" % t, "open %s.dbf shared" % t] + ["openindex %s" % f for f in extra]
        ops += run.ops + ["close"]
    big = Run(23, {"id": "id", "k": "k", "name": "name"}, keyspace=3000)
    for _ in range(4000):
        big.step(recall=False)
    ops += ["# big: a long random run", "open big.dbf shared"] + big.ops + ["close"]
    ops += ["tabletype vfp"]
    for t, seed, extra in (("v", 22, []), ("vi", 74, ["vbk.idx", "vnm.idx", "vseg.idx", "vdn.idx",
                                                                     "vuq.idx"])):
        run = Run(seed, VFP_FIELDS)
        for _ in range(700):
            run.step(dup_id=t == "v")
        ops += ["# %s: a random run" % t, "open %s.dbf shared" % t] + ["openindex %s" % f for f in extra]
        ops += run.ops + ["close"]
    write(out, setup, ops)


def write(out, setup, ops):
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "setup.txt"), "w") as f:
        f.write("\n".join(setup) + "\n")
    with open(os.path.join(out, "ops.txt"), "w") as f:
        f.write("\n".join(ops) + "\n")


if __name__ == "__main__":
    {"ntx": ntx, "cdx": cdx}[sys.argv[1]](sys.argv[2])
