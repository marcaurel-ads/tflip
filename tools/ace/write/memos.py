#!/usr/bin/env python3
"""Writes fixtures/write/memos/{setup,ops}.txt: memo, binary and NMemo
values written through the ACE API with block sizes 8, 64, 256 and 1024 and
in COMPRESSED fields, for tools/ace/write/run.sh.

The sizes are chosen around the .adm allocator's boundaries (slab chunks of
up to 32 blocks, extents rounded to their size class), then a seeded random
run updates, empties, deletes and appends; at the end come the cases of the
ACE API itself (a change cancelled, a record deleted with a change pending).

    tools/ace/write/memos.py fixtures/write/memos
"""
import os
import random
import sys

TABLES = [("m8", 8, ""), ("m64", 64, ""), ("m256", 256, ""), ("m1024", 1024, "")]
COMPRESSED = "c"
BLOCKS = [1, 2, 3, 5, 8, 13, 20, 31, 32, 33, 36, 40, 44, 60]


def text(size, mark):
    """`size` bytes of text, as the \\@ escape repeats a character."""
    if size == 0:
        return '""'
    return "%s\\@%05d%s" % (mark, size - 1, mark) if size > 1 else mark


def binary(size, seed):
    rng = random.Random(seed)
    return "".join("%02x" % rng.randrange(256) for _ in range(size))


def compressible(size):
    pattern = "compressible text "
    return (pattern * (size // len(pattern) + 1))[:size]


def incompressible(size, seed):
    rng = random.Random(seed)
    return "".join(rng.choice("abcdefghijklmnopqrstuvwxyz0123456789") for _ in range(size))


def main(out):
    setup = ["# Tables with memo, binary and NMemo fields, four block sizes and compressed."]
    for name, bs, _ in TABLES:
        size = "" if bs == 256 else " MEMOBLOCKSIZE %d" % bs
        setup.append("sql CREATE TABLE %s (id INTEGER, m MEMO, b BLOB, n NMEMO)%s" % (name, size))
    setup.append("sql CREATE TABLE %s (id INTEGER, m MEMO COMPRESSED, b BLOB COMPRESSED, "
                 "n NMEMO COMPRESSED)" % COMPRESSED)

    rng = random.Random(1)
    ops = []
    for name, bs, _ in TABLES:
        ops.append("# %s: block size %d" % (name, bs))
        ops.append("open %s.adt shared" % name)
        records = 0
        mark = iter("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ" * 20)

        def size_of(blocks):
            # Large blocks with fewer of them: the classes are counted in blocks.
            blocks = min(blocks, 44) if bs >= 256 else blocks
            return max(1, blocks * bs - rng.randrange(min(bs, 8)))

        # Appends with sizes around the boundaries.
        for blocks in BLOCKS:
            records += 1
            ops.append("append")
            ops.append("set id long %d" % records)
            ops.append("set m string %s" % text(size_of(blocks), next(mark)))
            ops.append("set b binary %s" % binary(min(size_of(max(1, blocks // 3)), 3000), records))
            ops.append("set n stringw %s" % text(size_of(blocks) // 2, next(mark)))
            ops.append("write")
        # A random run of changes.
        live = list(range(1, records + 1))
        for step in range(60):
            kind = rng.choice(["update", "update", "update", "null", "empty", "delete", "append"])
            if kind == "append" or not live:
                records += 1
                ops.append("append")
                ops.append("set id long %d" % records)
                ops.append("set m string %s" % text(size_of(rng.choice(BLOCKS)), next(mark)))
                ops.append("write")
                ops.append("state")
                ops.append("get id")
                live.append(None)
                continue
            target = rng.choice([r for r in live if r] or [1])
            ops.append("goto %d" % target)
            field = rng.choice(["m", "b", "n"])
            if kind == "delete":
                ops.append("delete")
                ops.append("write")
                live.remove(target)
                continue
            if kind == "null":
                ops.append("set %s null" % field)
            elif kind == "empty":
                ops.append("set %s %s \"\"" % (field, {"m": "string", "b": "binary", "n": "stringw"}[field]))
            elif field == "m":
                ops.append("set m string %s" % text(size_of(rng.choice(BLOCKS)), next(mark)))
            elif field == "n":
                ops.append("set n stringw %s" % text(size_of(rng.choice(BLOCKS)) // 2, next(mark)))
            else:
                ops.append("set b binary %s" % binary(min(size_of(rng.choice(BLOCKS[:9])), 3000), step))
            ops.append("write")
        # The API's own cases: a change cancelled, a delete with a change
        # pending, a value set twice before it is written.
        ops += ["top", "set m string %s" % text(size_of(5), "Q"), "set b binary %s" % binary(40, 99),
                "cancel", "get m", "top", "set m string %s" % text(size_of(3), "R"),
                "set m string %s" % text(size_of(40), "S"), "write", "skip 1",
                "set m string %s" % text(size_of(2), "T"), "delete", "write", "close"]

    ops.append("# %s: compressed fields" % COMPRESSED)
    ops.append("open %s.adt shared" % COMPRESSED)
    for i, size in enumerate([0, 1, 127, 128, 255, 256, 300, 510, 511, 600, 2000, 5000]):
        ops.append("append")
        ops.append("set id long %d" % (i + 1))
        ops.append("set m string %s" % (compressible(size) if size else '""'))
        ops.append("set b binary %s" % binary(size, i) if size else "set b null")
        ops.append("set n stringw %s" % (incompressible(size // 2, i) if size else '""'))
        ops.append("write")
        ops.append("get m")
    for i, size in enumerate([300, 128, 2000, 40]):
        ops += ["goto %d" % (i + 3), "set m string %s" % incompressible(size, 100 + i),
                "set n stringw %s" % compressible(size), "write"]
    ops.append("close")

    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "setup.txt"), "w") as f:
        f.write("\n".join(setup) + "\n")
    with open(os.path.join(out, "ops.txt"), "w") as f:
        f.write("\n".join(ops) + "\n")


if __name__ == "__main__":
    main(sys.argv[1])
