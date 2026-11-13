#!/usr/bin/env python3
"""Generates crates/tflip-ace/src/dbcs_tables.bin from the probed golden
fixtures/codepage/dbcs.json, which tools/ace/codepage/codepage2.c records.

The binary holds the four double-byte code pages' conversion tables for
AdsSetStringFromCodePage, in the layout crates/tflip-ace/src/dbcs.rs parses.
Run from the repo root after re-probing:

    tools/ace/codepage/gen_dbcs.py

All integers are little-endian. Layout:

    u16  number of pages
    per page:
      u32  code page
      256 x u16   single: the UTF-16 unit each byte converts to on its own
                  (a lead byte or an undefined byte is U+FFFD; byte 0 is 0)
      32 bytes    lead bitmap: bit b set when byte b is a lead byte (it and
                  the next byte make one character; it consumes two bytes)
      256 x u16   row index for each lead byte, 0xFFFF when the byte is not a
                  lead or is a lead that maps nothing (every trail U+FFFD)
      u16  number of rows
      rows x (256 x u16)   for each mapped lead, the unit of <lead, trail>
                           for trail 0x00..0xFF (U+FFFD where unmapped)
"""
import json
import os
import struct
import sys

PAGES = [932, 936, 949, 950]
FFFD = 0xFFFD


def main():
    here = os.path.abspath(__file__)
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(here))))
    src = os.path.join(root, "fixtures", "codepage", "dbcs.json")
    dst = os.path.join(root, "crates", "tflip-ace", "src", "dbcs_tables.bin")
    with open(src) as f:
        golden = json.load(f)
    by_cp = {p["code_page"]: p for p in golden["pages"]}

    out = bytearray()
    out += struct.pack("<H", len(PAGES))
    for cp in PAGES:
        pg = by_cp[cp]
        single = list(pg["single_u"])
        single[0] = 0  # a stored NUL reads back as a space; its value is U+0000
        lead = pg["lead_byte"]
        rows_by_lead = {r["lead"]: r["u"] for r in pg["leads"]}

        # Assign a row index to each lead byte that maps something.
        row_base = [0xFFFF] * 256
        rows = []
        for b in range(256):
            if lead[b] and b in rows_by_lead:
                row_base[b] = len(rows)
                rows.append(rows_by_lead[b])

        bitmap = bytearray(32)
        for b in range(256):
            if lead[b]:
                bitmap[b >> 3] |= 1 << (b & 7)

        out += struct.pack("<I", cp)
        out += struct.pack("<256H", *single)
        out += bitmap
        out += struct.pack("<256H", *row_base)
        out += struct.pack("<H", len(rows))
        for row in rows:
            out += struct.pack("<256H", *row)

    with open(dst, "wb") as f:
        f.write(out)
    print(f"wrote {dst} ({len(out)} bytes, {len(PAGES)} pages)", file=sys.stderr)


if __name__ == "__main__":
    main()
