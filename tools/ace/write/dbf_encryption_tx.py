#!/usr/bin/env python3
"""Writes fixtures/write/dbf_encryption_tx for tools/ace/write/run.sh: changes,
appends and deletes in a transaction on encrypted Clipper, FoxPro and Visual
FoxPro tables, rolled back and committed. The transactions need a remote
server (see run.sh's WRITE_SERVER_DIR).

    tools/ace/write/dbf_encryption_tx.py fixtures/write
"""
import os
import sys

KINDS = ("ntx", "cdx", "vfp")


def setup():
    out = ["# Free tables for the transactions on encrypted ones."]
    for kind in KINDS:
        out += [
            f"tabletype {kind}",
            f"sql CREATE TABLE t_{kind} (d1 CHAR(20), n1 INTEGER, m1 MEMO)",
            f"sql CREATE INDEX t_{kind}_d1 ON t_{kind} (d1)",
        ]
        for d1, n1, m1 in (("alpha", 1, "memo_alpha"), ("bravo", 2, "memo_bravo"), ("gamma", 3, "memo_gamma")):
            out.append(f"sql INSERT INTO t_{kind} VALUES ('{d1}', {n1}, '{m1}')")
    return "\n".join(out) + "\n"


def reads(count):
    out = []
    for recno in range(1, count + 1):
        out += [f"goto {recno}", "encstate", "get d1", "get n1", "get m1", "state"]
    return out


def ops():
    o = ["# Encrypted DBF tables in transactions on the remote server."]
    for kind in KINDS:
        t = f"t_{kind}.dbf"
        o += [f"tabletype {kind}", f"open {t} exclusive", "enableenc secret", "encrypttable", "close",
              f"open {t} shared", "enableenc secret", "begin",
              "goto 1", "set d1 string uno", "set m1 string changed_memo", "write",
              "append", "set d1 string four", "set n1 long 4", "set m1 string appended_memo", "write",
              "goto 2", "delete", "write", "flush", f"snapshot {kind}_during"]
        o += reads(4)
        o += ["rollback", "flush", f"snapshot {kind}_rolledback"] + reads(3)
        o += ["begin", "goto 1", "set d1 string eins", "set m1 string a_longer_memo_for_the_first_record_than_before", "write",
              "append", "set d1 string five", "set n1 long 5", "set m1 string five_memo", "write",
              "goto 3", "delete", "write", "commit", "flush", f"snapshot {kind}_committed"]
        o += reads(4) + ["close"]
    return "\n".join(o) + "\n"


folder = os.path.join(sys.argv[1], "dbf_encryption_tx")
os.makedirs(folder, exist_ok=True)
open(os.path.join(folder, "setup.txt"), "w").write(setup())
open(os.path.join(folder, "ops.txt"), "w").write(ops())
