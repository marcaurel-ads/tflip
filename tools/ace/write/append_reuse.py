#!/usr/bin/env python3
"""Writes fixtures/write/append_reuse/{setup,ops}.txt, for tools/ace/write/run.sh:
which deleted record an append of an ADT table takes (the free list's head,
or an appended one past the last), after deletes in various orders, a recall
that leaves the list's links, `AdsRecallAllRecords`, a pack, a zap, and
deletes and appends inside a transaction, committed and rolled back.

Each case has a table of its own, six records and an order on `id`; it
appends three records (`state` gives the record number each took).

    tools/ace/write/append_reuse.py fixtures/write/append_reuse
"""
import os
import sys

APPEND = ["append", "set id long 100", "write", "state",
          "append", "set id long 101", "write", "state",
          "append", "set id long 102", "write", "state", "rows"]


def delete(*slots):
    ops = []
    for slot in slots:
        ops += [f"goto {slot}", "delete", "write"]
    return ops


CASES = {
    "lifo": delete(2, 4, 3) + APPEND,
    "ascending": delete(1, 2, 3) + APPEND,
    "last": delete(6) + APPEND,
    "first_last": delete(1, 6) + APPEND,
    "pending": ["goto 2", "delete", "goto 3", "delete"] + APPEND,
    "recall_head": delete(2, 3) + ["goto 3", "recall"] + APPEND,
    "recall_tail": ["goto 2", "delete", "goto 3", "delete", "goto 4", "goto 2", "recall"] + APPEND,
    "recall_mid": delete(2, 3, 4) + ["goto 3", "recall"] + APPEND,
    "recallall": delete(2, 3) + ["recallall"] + APPEND,
    "pack": delete(2, 3) + ["pack"] + APPEND,
    "zap": ["zap"] + APPEND,
    "reindex": delete(2, 3) + ["reindex"] + APPEND,
    "cancel": delete(2, 3) + ["append", "set id long 99", "cancel"] + APPEND,
    "cancel_empty": ["append", "cancel"] + APPEND,
    "tx_commit": delete(2, 3) + ["begin"] + APPEND + ["commit", "rows"],
    "tx_delete_commit": ["begin"] + delete(2, 3) + ["commit"] + APPEND,
}


def main(out):
    setup, ops = [], []
    for name in CASES:
        setup.append(f"sql CREATE TABLE {name} (id INTEGER, name CHAR(8))")
        setup.append(f"sql CREATE INDEX by_id ON {name} (id)")
        for i in range(1, 7):
            setup.append(f"sql INSERT INTO {name} VALUES ({i}, 'r{i}')")
    for name, steps in CASES.items():
        ops.append(f"open {name}.adt exclusive")
        ops += steps
        ops += ["close"]
    # The same on a table other users have open: a record another handle has
    # locked, a delete another handle made.
    for name in ["shared_locked", "shared_other"]:
        setup.append(f"sql CREATE TABLE {name} (id INTEGER, name CHAR(8))")
        for i in range(1, 7):
            setup.append(f"sql INSERT INTO {name} VALUES ({i}, 'r{i}')")
    # Tables are numbered from 1 in the order they are opened.
    n = len(CASES) + 1
    ops += [f"open shared_locked.adt shared", f"open2 shared_locked.adt shared", f"use {n}"]
    ops += delete(2, 3) + [f"use {n + 1}", "goto 3", "lock 3", f"use {n}"] + APPEND
    ops += ["close", f"use {n + 1}", "close"]
    n += 2
    ops += ["open shared_other.adt shared", "open2 shared_other.adt shared", f"use {n + 1}"]
    ops += delete(4, 5) + [f"use {n}"] + APPEND + [f"use {n + 1}"] + APPEND
    ops += ["close", f"use {n}", "close"]
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "setup.txt"), "w") as f:
        f.write("\n".join(setup) + "\n")
    with open(os.path.join(out, "ops.txt"), "w") as f:
        f.write("\n".join(ops) + "\n")


if __name__ == "__main__":
    main(sys.argv[1])
