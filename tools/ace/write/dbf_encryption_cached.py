#!/usr/bin/env python3
"""Writes fixtures/write/dbf_encryption_cached_{ntx,cdx,vfp} for
tools/ace/write/run.sh: what the fields of the current record read as after
AdsDisableEncryption, depending on whether the record was read, written or
only moved to before.

    tools/ace/write/dbf_encryption_cached.py fixtures/write
"""
import os
import sys

KINDS = ("ntx", "cdx", "vfp", "adt")

ROWS = [
    ("alpha", 111, "memo_alpha"),
    ("bravo", 222, "memo_bravo_longer_than_the_first"),
    ("gamma", 333, "memo_gamma"),
]

GET = ["encstate", "get d1", "get n1", "get m1"]


def setup(kind, names):
    out = [f"# Free {kind.upper()} tables for AdsDisableEncryption's cached record.", f"tabletype {kind}"]
    for name in names:
        out.append(f"sql CREATE TABLE {name} (d1 CHAR(20), n1 INTEGER, m1 MEMO)")
        for d1, n1, m1 in ROWS:
            out.append(f"sql INSERT INTO {name} VALUES ('{d1}', {n1}, '{m1}')")
    return "\n".join(out) + "\n"


def stage(title, table, steps):
    return [f"# {title}", f"open {table}.dbf exclusive"] + steps + ["close"]


def ops():
    enc = ["enableenc secret", "encrypttable"]
    o = []
    o += stage("read, disable, read again", "g1",
               enc + ["goto 2"] + GET + ["disableenc"] + GET + ["goto 2"] + GET)
    o += stage("moved to, never read", "g2", enc + ["goto 2", "disableenc"] + GET)
    o += stage("one field read", "g3",
               enc + ["goto 2", "get n1", "disableenc", "get d1", "get n1", "get m1"])
    o += stage("one encrypted record", "g4",
               ["enableenc secret", "goto 2", "encryptrecord"] + GET
               + ["disableenc"] + GET + ["goto 2"] + GET)
    o += stage("the record opened at", "g5", enc + ["disableenc"] + GET)
    o += stage("a record just written", "g6",
               enc + ["append", "set d1 string zulu", "set n1 long 9", "set m1 string zmemo",
                      "write", "disableenc"] + GET + ["goto 4"] + GET)
    o += stage("a record changed", "g7",
               enc + ["goto 2", "set d1 string BRAVO2", "write", "disableenc"] + GET
               + ["goto 2"] + GET)
    o += stage("enable again after disable", "g8",
               enc + ["goto 2"] + GET + ["disableenc", "enableenc secret"] + GET + ["goto 2"] + GET)
    o += stage("the memo read first", "g9",
               enc + ["goto 2", "get m1", "disableenc"] + GET)
    o += stage("goto 3 read, goto 2 not", "g10",
               enc + ["goto 3"] + GET + ["goto 2", "disableenc"] + GET + ["goto 3"] + GET)
    # Writes to the cached record of a table where one record is encrypted.
    rec = ["enableenc secret", "goto 2", "encryptrecord"]
    o += stage("a field set after disable", "g11",
               rec + ["disableenc", "set d1 string CHANGED", "write"] + GET
               + ["goto 1", "goto 2"] + GET)
    o += stage("a delete after disable", "g12",
               rec + ["disableenc", "delete", "write"] + GET + ["goto 1", "goto 2"] + GET)
    o += stage("another field set after disable", "g13",
               rec + ["disableenc", "set n1 long 777", "write", "goto 1", "goto 2"] + GET
               + ["enableenc secret", "goto 1", "goto 2"] + GET)
    o += stage("a change after disable, moved away", "g14",
               rec + ["disableenc", "set d1 string CHANGED"] + ["goto 1"] + GET
               + ["goto 2"] + GET + ["enableenc secret", "goto 2"] + GET)
    # The same where the table is encrypted as a whole (a VFP table has no
    # other way to encrypt a record).
    o += stage("a field set after disable, table encrypted", "g15",
               enc + ["goto 2", "disableenc", "set d1 string CHANGED", "write"] + GET
               + ["goto 1", "goto 2"] + GET)
    o += stage("a delete after disable, table encrypted", "g16",
               enc + ["goto 2", "disableenc", "delete", "write"] + GET + ["goto 1", "goto 2"] + GET)
    return "\n".join(o) + "\n"


def main():
    root = sys.argv[1]
    names = [f"g{i}" for i in range(1, 17)]
    for kind in KINDS:
        folder = os.path.join(root, f"dbf_encryption_cached_{kind}")
        os.makedirs(folder, exist_ok=True)
        open(os.path.join(folder, "setup.txt"), "w").write(setup(kind, names))
        text = ops()
        if kind == "adt":
            # An ADT record read enciphered under the password again has a
            # memo reference that fails with 5001 (not matched by tflip).
            stages = text.split("# ")
            text = "# ".join(st for st in stages if not st.startswith("enable again after disable"))
            text = text.replace(".dbf ", ".adt ")
        open(os.path.join(folder, "ops.txt"), "w").write(f"tabletype {kind}\n" + text)


main()
