#!/usr/bin/env python3
"""Writes the scenarios of the RC4 encryption calls on free DBF tables for
tools/ace/write/run.sh: Clipper (NTX), FoxPro (CDX) and Visual FoxPro (VFP).

    tools/ace/write/dbf_encryption.py fixtures/write

makes fixtures/write/dbf_encryption_ntx, _cdx and _vfp. Every table (e1 to
e16, f1 to f4 without a memo field) has four records (the fourth deleted)
and an index on D1; each stage of ops.txt takes one of them through
AdsEnableEncryption (password `secret`), AdsEncryptTable and the rest,
reading back what the engine returns.
"""
import os
import sys

KINDS = ("ntx", "cdx", "vfp")

ROWS = [
    ("alpha", 111, "memo_alpha"),
    ("bravo", 222, "memo_bravo_longer_than_the_first"),
    ("gamma", 333, None),
    ("eps", 555, "memo_eps"),
]


def setup(kind, tables):
    out = [f"# Free {kind.upper()} tables for the RC4 encryption calls.", f"tabletype {kind}"]
    for name, memo in tables:
        fields = "d1 CHAR(20), n1 INTEGER" + (", m1 MEMO" if memo else "")
        out.append(f"sql CREATE TABLE {name} ({fields})")
        out.append(f"sql CREATE INDEX {name}_d1 ON {name} (d1)")
        for d1, n1, m1 in ROWS:
            if memo:
                m = "NULL" if m1 is None else f"'{m1}'"
                out.append(f"sql INSERT INTO {name} VALUES ('{d1}', {n1}, {m})")
            else:
                out.append(f"sql INSERT INTO {name} VALUES ('{d1}', {n1})")
        out.append(f"sql DELETE FROM {name} WHERE n1 = 555")
    return "\n".join(out) + "\n"


def reads(memo, recnos):
    out = ["encstate", "state"]
    for recno in recnos:
        out += [f"goto {recno}", "encstate", "get d1", "get n1"]
        if memo:
            out.append("get m1")
        out.append("state")
    return out


def change(memo):
    out = [
        "append",
        "set d1 string delta",
        "set n1 long 444",
    ]
    if memo:
        out.append("set m1 string memo_delta")
    out += [
        "write",
        "goto 2",
        "set d1 string BRAVO2",
    ]
    if memo:
        out.append("set m1 string a_much_longer_replacement_memo_value_that_needs_more_room_than_before_" + "x" * 80)
    out += ["write", "goto 1", "set d1 string alpha2", "write", "goto 3", "delete", "goto 4", "recall"]
    return out


def stage(title, table, memo, steps):
    return [f"# {title}", f"open {table}.dbf exclusive"] + steps + ["close"]


def ops(kind):
    NEWINDEX = "createindex i2.ntx i2 2 512 n1" if kind == "ntx" else "createindex - i2 2 512 n1"
    o = []
    # 1: only AdsEnableEncryption.
    o += stage("enable alone", "e1", True, ["enableenc secret"] + reads(True, [1, 2]))
    # 2: writes after it.
    o += stage("writes after enable", "e2", True,
               ["enableenc secret"] + change(True) + reads(True, [1, 2, 3, 4, 5]))
    # 3: records one at a time.
    o += stage("records", "e3", True,
               ["enableenc secret", "goto 1", "encryptrecord", "goto 2", "encryptrecord",
                "goto 1", "decryptrecord"] + reads(True, [1, 2, 3]))
    # 4: the table.
    o += stage("the table", "e4", True,
               ["enableenc secret", "encrypttable"] + reads(True, [1, 2, 3, 4]))
    # 5: writes in an encrypted table.
    o += stage("writes in an encrypted table", "e5", True,
               ["enableenc secret", "encrypttable"] + change(True) + reads(True, [1, 2, 3, 4, 5]))
    # 6: decrypt it.
    o += stage("decrypt", "e6", True,
               ["enableenc secret", "encrypttable", "decrypttable"] + reads(True, [1, 2]))
    # 7: disable.
    o += stage("disable", "e7", True,
               ["enableenc secret", "encrypttable", "disableenc"] + reads(True, [2, 3])
               + ["append", "set d1 string after_disable", "write"] + reads(True, [1]))
    # 8: pack.
    o += stage("pack", "e8", True,
               ["enableenc secret", "encrypttable", "pack"] + reads(True, [1, 2, 3]))
    # 9: zap.
    o += stage("zap", "e9", True,
               ["enableenc secret", "encrypttable", "zap", "append", "set d1 string zulu",
                "set n1 long 9", "set m1 string zmemo", "write"] + reads(True, [1]))
    # 10: reindex.
    o += stage("reindex", "e10", True,
               ["enableenc secret", "encrypttable", "reindex", "order e10_d1",
                "seek 2 bravo"] + reads(True, [2]))
    # 11: a new index on an encrypted table.
    o += stage("a new index", "e11", True,
               ["enableenc secret", "encrypttable", NEWINDEX, "order i2",
                "seek 2 222"] + reads(True, [2]))
    # 12: the same file opened again, without and with the password.
    o += ["# reopened", "open e12.dbf exclusive", "enableenc secret", "encrypttable", "close",
          "open e12.dbf exclusive"] + reads(True, [1, 2, 3])
    o += ["enableenc wrong", "enableenc secret"] + reads(True, [1, 2, 3, 4]) + ["close"]
    # 13: shared open of an encrypted table, then the password.
    o += ["# shared", "open e13.dbf exclusive", "enableenc secret", "encrypttable", "close",
          "open e13.dbf shared"] + reads(True, [1]) + change(True)[:6] + ["enableenc secret"]
    o += change(True) + reads(True, [1, 2, 5]) + ["close"]
    # 14: encrypt twice, decrypt twice, record calls without the password.
    o += stage("twice", "e14", True,
               ["encrypttable", "decrypttable", "encryptrecord", "enableenc secret",
                "encrypttable", "encrypttable", "decrypttable", "decrypttable",
                "decryptrecord", "encryptrecord"] + reads(True, [1]))
    # 15: a changed password.
    o += stage("another password", "e15", True,
               ["enableenc secret", "encrypttable", "enableenc other"] + reads(True, [1]))
    # 16: a delete of every record and a recall.
    o += stage("recall all", "e16", True,
               ["enableenc secret", "encrypttable", "goto 3", "delete", "recallall"]
               + reads(True, [1, 2, 3, 4]))
    # f: no memo field.
    o += stage("no memo: enable", "f1", False, ["enableenc secret"] + reads(False, [1]))
    o += stage("no memo: the table", "f2", False,
               ["enableenc secret", "encrypttable"] + reads(False, [1, 2, 4]))
    o += stage("no memo: writes", "f3", False,
               ["enableenc secret", "encrypttable"] + change(False) + reads(False, [1, 2, 3, 4, 5]))
    o += stage("no memo: decrypt", "f4", False,
               ["enableenc secret", "encrypttable", "decrypttable"] + reads(False, [1]))
    return "\n".join(o) + "\n"


def main():
    root = sys.argv[1]
    tables = [(f"e{i}", True) for i in range(1, 17)] + [(f"f{i}", False) for i in range(1, 5)]
    for kind in KINDS:
        folder = os.path.join(root, f"dbf_encryption_{kind}")
        os.makedirs(folder, exist_ok=True)
        open(os.path.join(folder, "setup.txt"), "w").write(setup(kind, tables))
        open(os.path.join(folder, "ops.txt"), "w").write(f"tabletype {kind}\n" + ops(kind))


main()
