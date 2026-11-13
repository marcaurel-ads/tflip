#!/usr/bin/env python3
"""Writes fixtures/write/reindex_pages/{setup,ops}.txt, for tools/ace/write/run.sh:
`AdsReindex61` with page sizes valid and not, on ADT tables whose structural
index has orders of several kinds, 200 records each, the index's pages 512
bytes (a table made by `CREATE INDEX`) or 2,048.

    tools/ace/write/reindex_pages.py fixtures/write/reindex_pages
"""
import os
import sys

# The page size given to AdsReindex61 (the index file's, as made, in the
# table's name: 512 unless a case says otherwise).
SIZES = [0, 1, 255, 256, 511, 512, 513, 1000, 1024, 1536, 2048, 4096, 8192, 8193, 16384,
         65536, 4294967295]


def main(out):
    setup, ops = [], []
    cases = [(f"p{i}", size, 512) for i, size in enumerate(SIZES)]
    cases += [("q0", 0, 2048), ("q1", 512, 2048), ("q2", 4096, 2048)]
    for name, _, page in cases:
        setup.append(f"sql CREATE TABLE {name} (id INTEGER, name CHAR(12), grp SHORT)")
        setup.append(f"sql CREATE INDEX by_id ON {name} (id)")
        setup.append(f"sql CREATE INDEX by_name ON {name} (name)")
        setup.append(
            f"sql EXECUTE PROCEDURE sp_CreateIndex90 ('{name}', '{name}', 'del', 'DELETED()', NULL, 4098, {page}, NULL)"
        )
        setup.append(
            f"sql EXECUTE PROCEDURE sp_CreateIndex90 ('{name}', '{name}', 'cust', 'name', NULL, 6, {page}, NULL)"
        )
        for i in range(200):
            setup.append(f"sql INSERT INTO {name} VALUES ({(i * 37) % 200}, 'n{(i * 91) % 200}', {i % 7})")
    for name, size, _ in cases:
        ops += [f"open {name}.adt exclusive", f"reindex61 {size}", "order -", "rows", "close"]
    # A table with no index file, and one opened shared.
    setup.append("sql CREATE TABLE plain (id INTEGER)")
    setup.append("sql INSERT INTO plain VALUES (1)")
    ops += ["open plain.adt exclusive", "reindex61 1024", "close"]
    ops += ["open p5.adt shared", "reindex61 1024", "reindex61 100", "close"]
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "setup.txt"), "w") as f:
        f.write("\n".join(setup) + "\n")
    with open(os.path.join(out, "ops.txt"), "w") as f:
        f.write("\n".join(ops) + "\n")


if __name__ == "__main__":
    main(sys.argv[1])
