#!/usr/bin/env python3
"""Writes fixtures/write/autoinc_reset/{setup,ops}.txt, for tools/ace/write/run.sh:
the next value of an ADT AutoInc field (and the header bytes that keep it)
after the table is emptied (`sp_ZapTable`, `AdsZapTable`, a `DELETE` of every
row, a pack after one) and when an Integer field becomes an AutoInc
(`ALTER TABLE`, `AdsRestructureTable`). Each case has a table of its own with
rows 1 to 5, appends the rows 'a' and 'b' after, and reads the values back.

    tools/ace/write/autoinc_reset.py fixtures/write/autoinc_reset
"""
import os
import sys

APPEND = ["append", "set name string a", "write", "state", "get id",
          "append", "set name string b", "write", "state", "get id", "rows"]

CASES = {
    "sp_zap": (["sql EXECUTE PROCEDURE sp_ZapTable('sp_zap')"], True),
    "delete_all": (["sql DELETE FROM delete_all"], True),
    "delete_all_pack": (["sql DELETE FROM delete_all_pack",
                         "sql EXECUTE PROCEDURE sp_PackTable('delete_all_pack')"], True),
    "delete_some": (["sql DELETE FROM delete_some WHERE id > 3"], True),
    "delete_last": (["sql DELETE FROM delete_last WHERE id = 5"], True),
    "pack_last": (["sql DELETE FROM pack_last WHERE id = 5",
                   "sql EXECUTE PROCEDURE sp_PackTable('pack_last')"], True),
    "api_zap": (None, True),
    "api_pack": (None, True),
    "api_delete": (None, True),
}


def main(out):
    setup, ops = ["sql CREATE TABLE junk (a INTEGER)"], []
    for name in CASES:
        setup.append(f"sql CREATE TABLE {name} (id AUTOINC, name CHAR(8))")
        for i in range(5):
            setup.append(f"sql INSERT INTO {name} (name) VALUES ('r{i}')")
    # Integer fields that become AutoInc: with values, with none, past the
    # table's own step, with a 0 and a duplicate.
    for name, rows in [("alter_values", [10, 20, 30]), ("alter_empty", []),
                       ("alter_zero", [0, 0, 7]), ("alter_dup", [3, 3, 3]),
                       ("rs_values", [10, 20, 30]), ("rs_empty", []),
                       ("rs_zero", [0, 0, 7]), ("rs_dup", [3, 3, 3])]:
        setup.append(f"sql CREATE TABLE {name} (id INTEGER, name CHAR(8))")
        for v in rows:
            setup.append(f"sql INSERT INTO {name} VALUES ({v}, 'r{v}')")
    for name, (steps, _) in CASES.items():
        if steps:
            ops += steps + ["sql INSERT INTO junk VALUES (1)"]
        ops.append(f"open {name}.adt exclusive")
        if name == "api_zap":
            ops += ["zap"]
        elif name == "api_pack":
            for i in range(1, 6):
                ops += [f"goto {i}", "delete"]
            ops += ["pack"]
        elif name == "api_delete":
            ops += ["goto 5", "delete", "write"]
        ops += APPEND + ["close"]
    for name in ["alter_values", "alter_empty", "alter_zero", "alter_dup"]:
        ops += [f"sql ALTER TABLE {name} ALTER COLUMN id id AUTOINC", "sql INSERT INTO junk VALUES (1)",
                f"open {name}.adt exclusive"]
        ops += APPEND + ["close"]
    for name in ["rs_values", "rs_empty", "rs_zero", "rs_dup"]:
        ops += [f"restructure {name}.adt 0 ||id,id,AutoInc", f"open {name}.adt exclusive"]
        ops += APPEND + ["close"]
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "setup.txt"), "w") as f:
        f.write("\n".join(setup) + "\n")
    with open(os.path.join(out, "ops.txt"), "w") as f:
        f.write("\n".join(ops) + "\n")


if __name__ == "__main__":
    main(sys.argv[1])
