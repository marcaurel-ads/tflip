#!/usr/bin/env python3
"""Turns locales.exe's output into crates/tflip-engine/src/system_collations.bin,
the static collations tflip derives from a host locale where it cannot ask
Windows (see crates/tflip-engine/src/system_collation.rs).

    wine locales.exe > locales.txt     # under Wine, any locale
    tools/ace/collation/gen_locales.py locales.txt > crates/tflip-engine/src/system_collations.bin

Layout (little-endian): u16 table count, then per table its 256 weights, 256
upper-case and 256 lower-case bytes; u16 locale count, then per locale its
u16 LCID, the u8 index of its table (0xFF: the default collation, whose
weights are the bytes themselves, for the language abbreviations ENU, ENG and
ENC), and its name (LOCALE_SNAME) as a u8 length and ASCII bytes.
"""
import struct, sys

IDENTITY = {"ENU", "ENG", "ENC"}
tables, index, locales = [], {}, []
for line in open(sys.argv[1]):
    lcid, name, abbrev, _acp, weights, upper, lower = line.split()
    if abbrev.upper() in IDENTITY:
        slot = 0xFF
    else:
        table = bytes.fromhex(weights) + bytes.fromhex(upper) + bytes.fromhex(lower)
        assert len(table) == 768
        if table not in index:
            index[table] = len(tables)
            tables.append(table)
        slot = index[table]
    locales.append((int(lcid, 16), slot, name.encode("ascii")))
assert len(tables) < 0xFF
out = sys.stdout.buffer
out.write(struct.pack("<H", len(tables)))
for table in tables:
    out.write(table)
out.write(struct.pack("<H", len(locales)))
for lcid, slot, name in sorted(locales):
    out.write(struct.pack("<HBB", lcid, slot, len(name)) + name)
