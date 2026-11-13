#!/usr/bin/env python3
"""Writes fixtures/write/nchar_indexes/{setup,ops}.txt: records written through
the ACE API to tables with ADI orders on NChar and NVarChar fields (keys that
are ICU sort keys), for tools/ace/write/run.sh.

The orders: NChar, NVarChar, NVarChar descending, NVarChar with a FOR
condition and a unique NChar one. A seeded random run appends, changes and
deletes records with text of Latin, Greek, Cyrillic, symbols, ideographs, Hangul, Thai and supplementary characters (spaces, NULL
and empty included), so that leaves split, empty and go on the free list; then
the table is packed and reindexed.

    tools/ace/write/nchar_indexes.py fixtures/write/nchar_indexes <icu_root.txt>
"""
import os
import random
import sys


def alphabet(icu_root):
    """Characters to draw text from: the ones tflip knows, by group."""
    groups = {"ascii": [], "latin": [], "greek": [], "cyrillic": [], "other": []}
    for line in open(icu_root):
        if line.startswith("#") or line.startswith("compressible") or not line.strip():
            continue
        word = line.split()
        if '+' in word[0] or word[1] == '?':
            continue
        code = int(word[0], 16)
        if code < 0x20 or 0x7F <= code < 0xA0 or code in (0xFFFE, 0xFFFF):
            continue
        if code < 0x7F:
            groups["ascii"].append(code)
        elif code < 0x250:
            groups["latin"].append(code)
        elif 0x370 <= code < 0x400:
            groups["greek"].append(code)
        elif 0x400 <= code < 0x530:
            groups["cyrillic"].append(code)
        elif code < 0x3000:
            groups["other"].append(code)
    # Weighed by formula or from a table of their own.
    groups["cjk"] = list(range(0x4E00, 0x9FFF, 37)) + list(range(0x3400, 0x4DBF, 53))
    groups["hangul"] = list(range(0xAC00, 0xD7A3, 41))
    groups["thai"] = list(range(0x0E01, 0x0E5B))
    groups["astral"] = list(range(0x20000, 0x2A6D6, 97)) + list(range(0x10000, 0x1000B))
    return groups


def main(out, icu_root):
    groups = alphabet(icu_root)
    rng = random.Random(11)

    def text(limit):
        n = min(rng.randrange(0, 13), limit)
        pool = rng.choice(["ascii", "ascii", "ascii", "latin", "greek", "cyrillic", "other", "mixed",
                           "cjk", "hangul", "thai", "astral"])
        chars = []
        for _ in range(n):
            if rng.random() < 0.12:
                chars.append(0x20)
                continue
            g = pool if pool != "mixed" else rng.choice(list(groups))
            chars.append(rng.choice(groups[g]))
        # Trailing blanks now and then.
        if rng.random() < 0.15:
            chars += [0x20] * rng.randrange(1, 3)
        chars = chars[:limit]
        # The limit is in UTF-16 units.
        while sum(2 if c > 0xFFFF else 1 for c in chars) > limit:
            chars.pop()
        return "".join(chr(c) for c in chars)

    def wide(s):
        return s.encode("utf-16-le").hex()

    setup = [
        "# NChar and NVarChar orders in a table's structural index.",
        "sql CREATE TABLE nx (id INTEGER, nc NCHAR(8), nv NVARCHAR(10), grp SHORT)",
        "sql CREATE INDEX by_nc ON nx (nc)",
        "sql CREATE INDEX by_nv ON nx (nv)",
        "sql CREATE INDEX by_nv_desc ON nx (nv DESC)",
        "sql EXECUTE PROCEDURE sp_CreateIndex90 ('nx', 'nx', 'cond', 'nv', 'grp > 3', 2, 512, NULL)",
        "sql CREATE TABLE nu (id INTEGER, code NCHAR(6))",
        "sql CREATE UNIQUE INDEX by_code ON nu (code)",
        "# Orders on several fields, whose segments are each the field's key.",
        "sql CREATE TABLE ny (id INTEGER, grp SHORT, nc NCHAR(4), nv NVARCHAR(5))",
        "sql CREATE INDEX by_grp_nc ON ny (grp, nc)",
        "sql CREATE INDEX by_nc_nv ON ny (nc, nv)",
        "sql CREATE INDEX by_nv_grp ON ny (nv, grp)",
    ]
    ops = ["open nx.adt shared"]
    next_id = 1

    def values():
        nonlocal next_id
        v = [("id", "long", str(next_id)), ("grp", "short", str(rng.randrange(8)))]
        next_id += 1
        for field, limit in (("nc", 8), ("nv", 10)):
            if rng.random() < 0.05:
                v.append((field, "null", ""))
            else:
                v.append((field, "stringu", wide(text(limit))))
        return v

    def put(v):
        for field, kind, value in v:
            ops.append(("set %s %s %s" % (field, kind, value)).rstrip())

    def append():
        ops.append("append")
        put(values())
        ops.extend(["write", "state"])

    for _ in range(400):
        append()
    ops.append("# Changes, deletes and appends that reuse the slots.")
    alive = list(range(1, 401))
    for _ in range(600):
        kind = rng.choice(["nc", "nv", "both", "grp", "delete", "delete", "append"])
        if kind == "append" or not alive:
            append()
            continue
        record = rng.choice(alive)
        ops.append("goto %d" % record)
        if kind == "delete":
            ops.extend(["delete", "write"])
            alive.remove(record)
            continue
        v = dict((f, (f, k, x)) for f, k, x in values())
        chosen = {"nc": ["nc"], "nv": ["nv"], "both": ["nc", "nv"], "grp": ["grp"]}[kind]
        put([v[f] for f in chosen])
        ops.append("write")
    ops.extend([
        "# A change cancelled, and one written by moving away.",
        "top", "set nv stringu %s" % wide("zzz cancel"), "cancel",
        "top", "set nv stringu %s" % wide("éclair"), "skip 1",
        "close",
        "# Reindexed and packed.",
        "open nx.adt exclusive",
        "reindex",
        "state",
        "close",
        "open nx.adt exclusive",
        "pack",
        "state",
        "close",
        "# Orders on several fields.",
        "open ny.adt shared",
    ])
    for _ in range(150):
        ops.append("append")
        ops.append("set id long %d" % next_id)
        next_id += 1
        ops.append("set grp short %d" % rng.randrange(4))
        for field, limit in (("nc", 4), ("nv", 5)):
            if rng.random() < 0.05:
                ops.append("set %s null" % field)
            else:
                ops.append("set %s stringu %s" % (field, wide(text(limit))))
        ops.extend(["write", "state"])
    alive = list(range(1, 151))
    for _ in range(150):
        record = rng.choice(alive)
        ops.append("goto %d" % record)
        if rng.random() < 0.3:
            ops.extend(["delete", "write"])
            alive.remove(record)
            continue
        ops.append("set nc stringu %s" % wide(text(4)))
        if rng.random() < 0.5:
            ops.append("set nv stringu %s" % wide(text(5)))
        ops.append("write")
    ops.extend([
        "close",
        "open ny.adt exclusive",
        "reindex",
        "close",
        "# Duplicate keys in a unique order.",
        "open nu.adt shared",
        "append", "set id long 1", "set code stringu %s" % wide("A1"), "write",
        "append", "set id long 2", "set code stringu %s" % wide("B2"), "write",
        "append", "set id long 3", "set code stringu %s" % wide("a1"), "write", "state", "cancel",
        "append", "set id long 3", "set code stringu %s" % wide("A1"), "write", "state", "cancel",
        "top", "set code stringu %s" % wide("B2"), "write", "state",
        "goto 2", "set code stringu %s" % wide("C3"), "write",
        "top", "set code stringu %s" % wide("B2"), "write",
        "close",
    ])
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "setup.txt"), "w") as f:
        f.write("\n".join(setup) + "\n")
    with open(os.path.join(out, "ops.txt"), "w") as f:
        f.write("\n".join(ops) + "\n")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
