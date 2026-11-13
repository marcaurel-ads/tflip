#!/usr/bin/env python3
"""Writes fixtures/write/uchar_keys/{setup,ops}.txt, for tools/ace/write/run.sh:
orders on NChar and NVarChar fields made with the key width options of
AdsCreateIndex (ADS_UCHAR_KEY_SHORT, _LONG and _XLONG), rows written through
them, and raw keys built with every AdsBuildRawKey100 option, among them
ADS_GET_PRIMARY_WEIGHT_LENGTH.

    tools/ace/write/uchar_keys.py fixtures/write/uchar_keys
"""
import os
import sys

SETUP = """\
# NChar and NVarChar fields and rows for orders built with the
# ADS_UCHAR_KEY_* options (the ops create them).
sql CREATE TABLE uk (id INTEGER, grp SHORT, nc NCHAR(4), nv NVARCHAR(7), c CHAR(5))
sql INSERT INTO uk VALUES (1, 2, 'abc', 'Zebra', 'x')
sql INSERT INTO uk VALUES (2, 1, 'été', 'apple', 'y')
sql INSERT INTO uk VALUES (3, 1, 'ÉTÉ', 'Ömega', 'z')
sql INSERT INTO uk VALUES (4, 3, 'éééé', 'ÉÉÉÉÉÉÉ', 'w')
sql INSERT INTO uk VALUES (5, 3, 'ABCD', 'abcdefg', 'v')
sql INSERT INTO uk VALUES (6, 2, NULL, NULL, NULL)
sql INSERT INTO uk VALUES (7, 2, '', '', '')
sql CREATE INDEX by_nc ON uk (nc)
"""

SHORT, LONG, XLONG = 0x20000000, 0x40000000, 0x60000000

ORDERS = [
    ("nc_s", "nc", SHORT), ("nc_l", "nc", LONG), ("nc_x", "nc", XLONG),
    ("nv_s", "nv", SHORT), ("nv_l", "nv", LONG), ("nv_x", "nv", XLONG),
    ("c_x", "c", XLONG), ("id_s", "id", SHORT),
    ("ncnv_s", "nc;nv", SHORT), ("ncnv_x", "nc;nv", XLONG),
    ("grpnc_l", "grp;nc", LONG), ("ncid_s", "nc;id", SHORT), ("idnv_x", "id;nv", XLONG),
    ("nc_sd", "nc", SHORT | 8), ("nv_lu", "nv", LONG | 1), ("nc_sc", "nc", SHORT | 2),
    ("ncc", "nc;c", SHORT), ("cnc", "c;nc", LONG),
    ("idc", "id;c", 0), ("idgrp", "id;grp", 0), ("idnv_d", "id;nv", 0), ("idnc_s", "id;nc", SHORT),
    ("grpid", "grp;id", 0),
]

RAW_ORDERS = ["by_nc", "nc_s", "nc_x", "nv_s", "nv_l", "ncnv_s", "ncnv_x", "ncid_s",
              "grpnc_l", "c_x", "id_s", "idnv_x", "nc_sd", "ncc", "cnc",
              "idc", "idgrp", "idnv_d", "idnc_s", "grpid"]
RAW_VALUES = ["nc=ab", "nc=ét", "nc=ABCD", "nc=", "nv=ab", "nv=étag", "nv=",
              "c=ab", "id=7", "grp=1;nc=ab", "nc=ab;nv=cd", "nc=ab;id=7", "id=7;nv=ab",
              "nc=a;nv=b", "nc=ab;c=cd", "c=cd;nc=ab", "nv=cd;nc=ab", "grp=1", "id=7;c=ab", "id=7;grp=1"]


def main(out):
    ops = ["# Orders made with the key width options.", "open uk.adt exclusive"]
    for tag, expr, options in ORDERS:
        ops.append(f"createindex - {tag} {options} 0 {expr}")
    ops += [
        "createindex - nc_c %d 0 nc FOR grp > 1" % SHORT,
        "createindex uk2.adi nv_f %d 512 nv" % LONG,
        "createindex uk2.adi nc_ff 4294967295 512 nc",
        "createindex - nc_bad 134217728 0 nc",
        "# Rows written through the orders, then the orders rebuilt.",
        "append", "set id long 8", "set grp short 1",
        "set nc stringu 3300e9007a00", "set nv stringu 7a00e9006500", "set c string q",
        "write", "top", "set nc stringu 5900", "write", "skip 2", "delete", "write",
        "close", "open uk.adt exclusive", "reindex", "pack", "close",
        "open uk.adt shared",
    ]
    for tag in ["by_nc"] + [o[0] for o in ORDERS if o[0] not in ("c_x", "id_s", "nc_sc")]:
        ops += [f"order {tag}", "rows", "seek 1 e", "seek 1 ét"]
    ops.append("# Raw keys of every length option.")
    for tag in RAW_ORDERS:
        for option in [0, 1, 2, 4, 3, 5, 6, 8, 16, 2147483648]:
            for values in RAW_VALUES:
                if option in (0, 2, 4) or values in ("nc=ab", "nv=ab", "nc=ab;nv=cd", "c=ab"):
                    ops.append(f"rawkey {tag} {option} {values}")
    ops.append("close")
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "setup.txt"), "w", encoding="utf-8") as f:
        f.write(SETUP)
    with open(os.path.join(out, "ops.txt"), "w", encoding="utf-8") as f:
        f.write("\n".join(ops) + "\n")


if __name__ == "__main__":
    main(sys.argv[1])
