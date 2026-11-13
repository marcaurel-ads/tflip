#!/usr/bin/env python3
"""Writes fixtures/write/indexes/{setup,ops}.txt: records written through the
ACE API to tables with ADI orders of many kinds, for tools/ace/write/run.sh.

The orders (512-byte pages, so that trees grow several levels): a field, a
descending field, two fields, an expression (UPPER), a FOR condition, a
custom order and a binary DELETED() order; a second table has a unique
order. A seeded random run appends, changes and deletes records, so that
leaves split, empty and go on the free list, and pages come back from it.

    tools/ace/write/indexes.py fixtures/write/indexes
"""
import os
import random
import sys

WORDS = ("apple banana cherry damson elder fig grape hazel ilama jujube kiwi lemon mango nectar "
         "olive papaya quince raisin satsuma tamarind ugli vanilla walnut xigua yam zucchini").split()


def main(out):
    setup = [
        "# Orders of many kinds in a table's structural index, 512-byte pages.",
        "sql CREATE TABLE ix (id INTEGER, name CHAR(12), grp SHORT, amount NUMERIC(8, 2), d DATE)",
        "sql CREATE INDEX by_id ON ix (id)",
        "sql CREATE INDEX by_name ON ix (name)",
        "sql CREATE INDEX by_grp_desc ON ix (grp DESC)",
        "sql CREATE INDEX by_name_id ON ix (name, id)",
        "sql EXECUTE PROCEDURE sp_CreateIndex90 ('ix', 'ix', 'upper', 'UPPER(name)', NULL, 2, 512, NULL)",
        "sql EXECUTE PROCEDURE sp_CreateIndex90 ('ix', 'ix', 'cond', 'amount', 'grp > 3', 2, 512, NULL)",
        "sql EXECUTE PROCEDURE sp_CreateIndex90 ('ix', 'ix', 'cust', 'name', NULL, 6, 512, NULL)",
        "sql EXECUTE PROCEDURE sp_CreateIndex90 ('ix', 'ix', 'deleted', 'DELETED()', NULL, 4098, 512, NULL)",
        "# A unique order.",
        "sql CREATE TABLE u (id INTEGER, code CHAR(6))",
        "sql CREATE UNIQUE INDEX by_code ON u (code)",
    ]
    rng = random.Random(7)
    ops = ["open ix.adt shared"]
    live = []
    next_id = 1

    def record_values():
        nonlocal next_id
        name = rng.choice(WORDS) + str(rng.randrange(100))
        values = [("id", "long", str(next_id)), ("name", "string", name),
                  ("grp", "short", str(rng.randrange(8))),
                  ("amount", "double", "%.2f" % (rng.randrange(-100000, 100000) / 100)),
                  ("d", "julian", str(2458000 + rng.randrange(400)))]
        next_id += 1
        return values

    def append():
        ops.append("append")
        for field, kind, value in record_values():
            ops.append("set %s %s %s" % (field, kind, value))
        ops.append("write")
        ops.append("state")
        live.append(None)

    for _ in range(500):
        append()
    ops.append("# Changes, deletes and appends that reuse the slots.")
    slots = 500
    alive = list(range(1, slots + 1))
    for step in range(700):
        kind = rng.choice(["name", "grp", "both", "delete", "delete", "append"])
        if kind == "append" or not alive:
            append()
            continue
        record = rng.choice(alive)
        ops.append("goto %d" % record)
        if kind == "delete":
            ops += ["delete", "write"]
            alive.remove(record)
            continue
        if kind in ("name", "both"):
            ops.append("set name string %s%d" % (rng.choice(WORDS), rng.randrange(100)))
        if kind in ("grp", "both"):
            ops.append("set grp short %d" % rng.randrange(8))
            ops.append("set amount double %.2f" % (rng.randrange(-100000, 100000) / 100))
        ops.append("write")
    ops += [
        "# A change cancelled, and one written by moving away.",
        "top", "set name string zzzcancelled", "cancel",
        "top", "set name string aaafirst", "skip 1",
        "append", "set id long 99999", "cancel",
        "close",
        "# Duplicate keys in a unique order.",
        "open u.adt shared",
        "append", "set id long 1", "set code string A1", "write",
        "append", "set id long 2", "set code string B2", "write",
        "append", "set id long 3", "set code string A1", "write", "state", "cancel",
        "top", "set code string B2", "write", "state",
        "goto 2", "set code string C3", "write",
        "top", "set code string B2", "write",
        "close",
    ]
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "setup.txt"), "w") as f:
        f.write("\n".join(setup) + "\n")
    with open(os.path.join(out, "ops.txt"), "w") as f:
        f.write("\n".join(ops) + "\n")


if __name__ == "__main__":
    main(sys.argv[1])
