#!/usr/bin/env python3
"""Writes fixtures/write/binary_changes/{setup,ops}.txt, for tools/ace/write/run.sh:
the writes that change a binary DELETED() order's bits (deletes and recalls,
written by a write, by moving off the record or by `reindex`), and what the
`AdsReindex` that follows leaves in the tag header's change count (0x08).

Each case has a table of its own with four records, an order on `id` and a
binary order. A case first walks the binary order (`rows`, which shows
whether its bits followed the writes), then reindexes and walks it again.

    tools/ace/write/binary_changes.py fixtures/write/binary_changes
"""
import os
import sys

CASES = {
    # A delete or recall of the current record is pending until it is
    # written or the record is left.
    "del": ["goto 2", "delete"],
    "del_recall": ["goto 2", "delete", "recall"],
    "del_twice": ["goto 2", "delete", "goto 2", "delete"],
    "del_two": ["goto 2", "delete", "goto 3", "delete"],
    "del_two_recall": ["goto 2", "delete", "goto 3", "delete", "goto 2", "recall"],
    "del_move_recall": ["goto 2", "delete", "goto 3", "goto 2", "recall"],
    "del_regoto_recall": ["goto 2", "delete", "goto 2", "recall", "goto 2", "delete"],
    "del_regoto_recall_two": [
        "goto 2", "delete", "goto 2", "recall", "goto 3", "delete", "goto 3", "recall",
    ],
    "del_write": ["goto 2", "delete", "goto 2", "write"],
    "recall_noop": ["goto 2", "recall", "goto 2", "recall"],
    # A recall or a delete of a deleted record gone to (and refused, 5022).
    "landed_recall_twice": ["goto 2", "delete", "goto 3", "goto 2", "recall", "recall"],
    "landed_recall_write": ["goto 2", "delete", "goto 3", "goto 2", "recall", "write"],
    "del_all": ["goto 1", "delete", "goto 2", "delete", "goto 3", "delete", "goto 4", "delete"],
    "del_recall_all": [
        "goto 1", "delete", "goto 2", "delete", "goto 3", "delete", "goto 4", "delete",
        "goto 1", "recall", "goto 2", "recall", "goto 3", "recall", "goto 4", "recall",
    ],
    "recallall_none": ["recallall"],
    "del_recallall": ["goto 2", "delete", "recallall"],
    "del_two_recallall": ["goto 2", "delete", "goto 3", "delete", "recallall"],
    # Writes that leave the bits alone.
    "upd_nokey": ["goto 2", "set name string zz", "write"],
    "upd_key": ["goto 2", "set id long 99", "write"],
    "upd_key_twice": ["goto 2", "set id long 99", "write", "set id long 98", "write"],
    "append": ["append", "set id long 9", "write"],
    "append_two": ["append", "set id long 9", "write", "append", "set id long 8", "write"],
    "append_cancel": ["append", "set id long 9", "cancel"],
    # A delete or recall written, then undone or repeated.
    "del_write_recall": ["goto 2", "delete", "write", "recall"],
    "del_write_recall_write": ["goto 2", "delete", "write", "recall", "write"],
    "del_write_recall_move": ["goto 2", "delete", "write", "recall", "goto 3"],
    "del_write_regoto_recall": ["goto 2", "delete", "write", "goto 2", "recall", "write"],
    "del_write_recall_del": ["goto 2", "delete", "write", "recall", "write", "delete", "write"],
    "del_write_recall_next_del": [
        "goto 2", "delete", "write", "recall", "goto 3", "delete", "write",
    ],
    "del_upd_write": ["goto 2", "delete", "set name string x", "write"],
    "del_upd_key_write": ["goto 2", "delete", "set id long 77", "write"],
    "upd_key_del_write": ["goto 2", "set id long 77", "delete", "write"],
    "del_upd_recall": [
        "goto 2", "delete", "goto 3", "set name string x", "write", "goto 2", "recall",
    ],
    "many": [
        "goto 1", "delete", "goto 1", "recall", "goto 2", "set id long 50", "write",
        "set name string q", "write", "append", "set id long 7", "set name string n", "write",
        "goto 4", "delete", "goto 4", "recall", "goto 4", "delete",
    ],
}


def main(out):
    setup, ops = [], []
    for name in CASES:
        setup.append(f"sql CREATE TABLE {name} (id INTEGER, name CHAR(12))")
        setup.append(f"sql CREATE INDEX by_id ON {name} (id)")
        setup.append(
            f"sql EXECUTE PROCEDURE sp_CreateIndex90 ('{name}', '{name}', 'del', 'DELETED()', NULL, 4098, 512, NULL)"
        )
        for i in range(1, 5):
            setup.append(f"sql INSERT INTO {name} VALUES ({i}, 'r{i}')")
    for name, steps in CASES.items():
        ops.append(f"open {name}.adt exclusive")
        ops += steps
        ops += ["order del", "rows", "reindex", "rows", "close"]
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "setup.txt"), "w") as f:
        f.write("\n".join(setup) + "\n")
    with open(os.path.join(out, "ops.txt"), "w") as f:
        f.write("\n".join(ops) + "\n")


if __name__ == "__main__":
    main(sys.argv[1])
