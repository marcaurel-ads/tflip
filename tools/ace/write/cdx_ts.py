#!/usr/bin/env python3
"""Writes fixtures/write/cdx_timestamps/setup.txt and ops.txt: CDX orders on
expressions whose value is a timestamp (CREATETIMESTAMP, STOTS, CTOTS) on a
Visual FoxPro table (some on fields that take NULL) and a FoxPro table, kept
current through a seeded random run of appends, changes, deletes and
recalls, then walked, sought and looked up; orders built on the tables'
records after.

    tools/ace/write/cdx_ts.py fixtures/write/cdx_timestamps
"""
import calendar
import os
import random
import sys

out = sys.argv[1]
rng = random.Random(1717)

TS = ["y", "mo", "dd", "h", "mi", "s", "ms"]
CTS = "CREATETIMESTAMP(y, mo, dd, h, mi, s, ms)"

setup = [
    "# A Visual FoxPro table with orders on timestamp expressions, some of fields",
    "# that take NULL (whose keys then start with a NULL byte), and a FoxPro one.",
    "tabletype vfp",
    "createtable v.dbf 0 id,Integer;y,Integer;mo,Integer;dd,Integer;h,Integer;mi,Integer;"
    "s,Integer;ms,Integer;yn,Integer,NULL;d,Date;dn,Date,NULL;c,Character,30;cn,Character,30,NULL",
    f"createindex - cts 2 0 {CTS}",
    f"createindex - dcts 10 0 {CTS}",
    f"createindex - fcts 2 0 {CTS} FOR id > 100",
    "createindex - ucts 3 0 CREATETIMESTAMP(y, mo, dd, h, mi, s, 0)",
    "createindex - ncts 2 0 CREATETIMESTAMP(yn, mo, dd, h, mi, s, ms)",
    "createindex - stots 2 0 STOTS(DTOS(d))",
    "createindex - nstots 2 0 STOTS(DTOS(dn))",
    "createindex - ctots 2 0 CTOTS(c)",
    "createindex - nctots 2 0 CTOTS(cn)",
    "createindex - ucn 2 0 UPPER(cn)",
    "close",
    "tabletype cdx",
    "createtable f.dbf 0 id,Numeric,6;y,Numeric,6;mo,Numeric,4;dd,Numeric,4;h,Numeric,4;"
    "mi,Numeric,4;s,Numeric,4;ms,Numeric,6;d,Date;c,Character,30",
    f"createindex - cts 2 0 {CTS}",
    f"createindex - dcts 10 0 {CTS}",
    "createindex - stots 2 0 STOTS(DTOS(d))",
    "createindex - ctots 2 0 CTOTS(c)",
    "close",
]

# Timestamps drawn again now and then, so that keys repeat, also to the
# second with other milliseconds.
pool = []


def timestamp():
    if pool and rng.random() < 0.3:
        y, mo, dd, h, mi, s, ms = rng.choice(pool)
        if rng.random() < 0.5:
            ms = rng.randrange(1000)
        return (y, mo, dd, h, mi, s, ms)
    y = rng.choice([rng.randrange(1900, 2100)] * 8 + [1, 100, 1752, 1899, 9999, 2000])
    mo = rng.randrange(1, 13)
    dd = rng.randrange(1, calendar.monthrange(y, mo)[1] + 1)
    h, mi, s = rng.randrange(24), rng.randrange(60), rng.randrange(60)
    ms = rng.choice([rng.randrange(1000), 0, 499, 500, 999])
    if rng.random() < 0.08:
        h, mi, s, ms = 23, 59, 59, rng.choice([0, 500, 999])
    if rng.random() < 0.05:
        h, mi, s, ms = 0, 0, 0, 0
    value = (y, mo, dd, h, mi, s, ms)
    pool.append(value)
    return value


def text(value):
    y, mo, dd, h, mi, s, ms = value
    return rng.choice([
        f"{mo:02d}/{dd:02d}/{y:04d} {h:02d}:{mi:02d}:{s:02d}",
        f"{mo:02d}/{dd:02d}/{y:04d} {h:02d}:{mi:02d}:{s:02d}.{ms:03d}",
        f"{mo:02d}/{dd:02d}/{y:04d}",
    ])


def date(value):
    y, mo, dd = value[:3]
    return f"{mo:02d}/{dd:02d}/{y:04d}"


def fields(visual, everything):
    """Sets on the current record: the timestamp's parts, and (some of) the
    other fields."""
    ops = []
    value = timestamp()
    if everything or rng.random() < 0.6:
        for name, part in zip(TS, value):
            ops.append(f"set {name} long {part}")
    if visual and (everything or rng.random() < 0.4):
        ops.append("set yn null" if rng.random() < 0.2 else f"set yn long {value[0]}")
    if everything or rng.random() < 0.4:
        ops.append("set d empty" if rng.random() < 0.1 else f"set d date {date(timestamp())}")
    if visual and (everything or rng.random() < 0.4):
        r = rng.random()
        ops.append("set dn null" if r < 0.15 else "set dn empty" if r < 0.25
                   else f"set dn date {date(timestamp())}")
    if everything or rng.random() < 0.5:
        ops.append("set c empty" if rng.random() < 0.08 else f"set c string {text(timestamp())}")
    if visual and (everything or rng.random() < 0.5):
        r = rng.random()
        ops.append("set cn null" if r < 0.15 else "set cn empty" if r < 0.2
                   else f"set cn string {text(timestamp())}")
    return ops


def run(table, visual, appends, changes):
    ops = [f"tabletype {'vfp' if visual else 'cdx'}", f"open {table}"]
    records, deleted = 0, set()
    for _ in range(appends):
        records += 1
        ops += ["append", f"set id long {records}"] + fields(visual, True) + ["write"]
    for _ in range(changes):
        r = rng.random()
        if r < 0.15:
            records += 1
            ops += ["append", f"set id long {records}"] + fields(visual, True) + ["write"]
            continue
        record = rng.randrange(1, records + 1)
        ops.append(f"goto {record}")
        if r < 0.25 and record not in deleted:
            ops.append("delete")
            deleted.add(record)
        elif r < 0.32 and record in deleted:
            ops.append("recall")
            deleted.discard(record)
        else:
            ops += fields(visual, False) + ["write"]
    return ops, records


def reads(tags, records, visual):
    ops = []
    for record in sorted(rng.sample(range(1, records + 1), 25)):
        ops.append(f"goto {record}")
        ops += [f"keyfromrecord {tag}" for tag in tags]
    for tag in tags:
        ops += [f"order {tag}", "rows"]
    # Hard and soft seeks of the timestamps the records were given (with
    # and without their milliseconds), of times between them and of dates.
    for tag in ["cts", "dcts", "ctots"] + (["nctots", "fcts"] if visual else []):
        ops.append(f"order {tag}")
        for _ in range(12):
            value = rng.choice(pool)
            y, mo, dd, h, mi, s, ms = value
            seek = rng.choice([
                f"{date(value)} {h:02d}:{mi:02d}:{s:02d}",
                f"{date(value)} {h:02d}:{mi:02d}:{s:02d}.{ms:03d}",
                f"{date(value)}",
                f"{date(value)} {h:02d}:{mi:02d}",
            ])
            ops += [f"seek {rng.choice([1, 2])} {seek}", "pos"]
        ops += ["seek 1 ", "pos", "seek 2 ", "pos"]
    # AdsLookupKey of doubles (days and their fraction) and of text.
    for tag in ["cts", "stots"]:
        for _ in range(6):
            y, mo, dd, h, mi, s, ms = rng.choice(pool)
            julian = calendar_julian(y, mo, dd)
            ops.append(f"lookup {tag} 4 {julian + (h * 3600 + mi * 60 + s) / 86400:.17g}")
            ops.append(f"lookup {tag} 4 {julian}")
            ops.append(f"lookup {tag} 2 {mo:02d}/{dd:02d}/{y:04d} {h:02d}:{mi:02d}:{s:02d}")
    ops.append("order -")
    return ops


def calendar_julian(y, mo, dd):
    a = -1 if mo < 3 else 0
    return (1461 * (y + 4800 + a)) // 4 + (367 * (mo - 2 - 12 * a)) // 12 \
        - (3 * ((y + 4900 + a) // 100)) // 4 + dd - 32075


ops = ["# A seeded random run on the VFP table."]
run_ops, records = run("v.dbf", True, 260, 220)
ops += run_ops
ops += reads(["cts", "dcts", "fcts", "ucts", "ncts", "stots", "nstots", "ctots", "nctots", "ucn"],
             records, True)
ops += [
    "# Orders built on the records, then kept current.",
    "createindex - bnct 2 0 CTOTS(cn) FOR id < 200",
    "createindex - bds 10 0 STOTS(DTOS(dn))",
    f"createindex - bcts 2 0 {CTS}",
]
for _ in range(15):
    records += 1
    ops += ["append", f"set id long {records}"] + fields(True, True) + ["write"]
for tag in ["bnct", "bds", "bcts"]:
    ops += [f"order {tag}", "rows"]
ops += ["order -", "close"]

ops.append("# The same on the FoxPro table.")
run_ops, records = run("f.dbf", False, 160, 120)
ops += run_ops
ops += reads(["cts", "dcts", "stots", "ctots"], records, False)
ops += [f"createindex - bcts 2 0 {CTS}"]
for _ in range(10):
    records += 1
    ops += ["append", f"set id long {records}"] + fields(False, True) + ["write"]
ops += ["order bcts", "rows", "order -", "close"]

# A FoxPro order on a timestamp built on records leaves out the keys'
# trailing zeros, but the engine reads it and keeps it current as if they
# were spaces: a built key that ends in zeros is not found, one that ends
# in a space keeps it as later keys come into its leaf. The same keys in an
# order kept current from the start are found.
setup += [
    "tabletype cdx",
    "createtable g.dbf 0 id,Numeric,6;c,Character,30",
    "createindex - m 2 0 CTOTS(c)",
    "close",
]
ops += ["# A FoxPro order on a timestamp built on records.", "tabletype cdx", "open g.dbf"]
built = ["01/02/2020 12:00:00", "01/03/2020 06:00:00", "01/04/2020 12:34:56",
         "01/01/2020 12:00:00", "01/02/2020 00:01:41", "01/02/2020 00:10:10"]
later = ["01/02/2020 18:00:00", "01/02/2020 00:00:01", "01/02/2020 00:07:24"]
for i, value in enumerate(built + later, 1):
    if i == len(built) + 1:
        ops.append("createindex - b 2 0 CTOTS(c)")
    ops += ["append", f"set id long {i}", f"set c string {value}", "write"]
for tag in ["b", "m"]:
    ops += [f"order {tag}", "rows"]
    for value in built + later:
        ops += [f"seek 2 {value}", "pos"]
    ops += [f"lookup {tag} 2 01/02/2020 12:00:00", f"lookup {tag} 4 2458851.5",
            f"lookup {tag} 1 c142c27180000000", f"lookup {tag} 1 c142c27180202020"]
ops += ["order -", "close"]

os.makedirs(out, exist_ok=True)
with open(os.path.join(out, "setup.txt"), "w") as f:
    f.write("\n".join(setup) + "\n")
with open(os.path.join(out, "ops.txt"), "w") as f:
    f.write("\n".join(ops) + "\n")
